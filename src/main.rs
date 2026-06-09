use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Terminal,
};
use std::{
    collections::VecDeque,
    error::Error,
    fs::File,
    io::{self, Write},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

// --- Color Palette Constants ---
const COLOR_PRIMARY: Color = Color::Yellow;
const COLOR_SECONDARY: Color = Color::Magenta;
const COLOR_ACCENT: Color = Color::Cyan;
const COLOR_BG_DARK: Color = Color::Black;

/// Defines the input state of the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputMode {
    Normal,
    Editing,
}

/// Messages emitted from background execution threads.
#[derive(Debug)]
enum NetworkEvent {
    ConnectionSuccess(String),
    ConnectionFailed { ssid: String, error: String },
    ScanComplete(Vec<WifiNetwork>),
    ScanFailed,
}

/// Structured data representing a discovered Wi-Fi network access point.
#[derive(Debug, Clone)]
struct WifiNetwork {
    display_string: String,
    ssid: String,
    is_secured: bool,
}

/// The core Application State Manager.
struct NetworkManagerApp {
    networks: Vec<WifiNetwork>,
    list_state: ListState,
    logs: VecDeque<String>,
    event_tx: Sender<NetworkEvent>,
    event_rx: Receiver<NetworkEvent>,
    input_mode: InputMode,
    password_input: String,
    target_ssid: String,
    is_busy: bool,
}

impl NetworkManagerApp {
    /// Instantiates the core application and fires off the initial background network scan.
    fn new() -> Self {
        let mut logs = VecDeque::new();
        logs.push_back("Initializing Network Manager...".to_string());

        let (event_tx, event_rx) = mpsc::channel();

        let mut app = Self {
            networks: Vec::new(),
            list_state: ListState::default(),
            logs,
            event_tx,
            event_rx,
            input_mode: InputMode::Normal,
            password_input: String::new(),
            target_ssid: String::new(),
            is_busy: false,
        };

        app.scan_networks(false);
        app
    }

    /// Appends a message onto the sliding console logs view.
    fn log_message(&mut self, message: &str) {
        self.logs.push_back(message.to_string());
        if self.logs.len() > 100 {
            self.logs.pop_front();
        }
    }

    /// Spawns a thread to discover available local Wi-Fi networks using nmcli.
    fn scan_networks(&mut self, force_hardware_rescan: bool) {
        if self.is_busy {
            self.log_message("[WARN] An operation is already in progress. Please wait.");
            return;
        }

        self.is_busy = true;
        if force_hardware_rescan {
            self.log_message("Requesting hardware Wi-Fi rescan...");
        } else {
            self.log_message("Fetching available networks...");
        }

        let tx = self.event_tx.clone();
        std::thread::spawn(move || {
            if force_hardware_rescan {
                let _ = Command::new("nmcli")
                    .args(["device", "wifi", "rescan"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .output();
            }

            let output = Command::new("nmcli")
                .args(["-t", "-e", "yes", "-f", "SSID,SIGNAL,BARS,SECURITY", "device", "wifi", "list"])
                .stderr(Stdio::null())
                .output();

            if let Ok(out) = output {
                let raw_stdout = String::from_utf8_lossy(&out.stdout);
                let mut discovered = Vec::new();

                for line in raw_stdout.lines() {
                    if line.trim().is_empty() || line.starts_with(':') {
                        continue;
                    }

                    let parts: Vec<&str> = line.split(':').collect();
                    if parts.len() >= 4 && !parts[0].is_empty() {
                        let ssid = parts[0].replace("\\:", ":");
                        let signal = parts[1];
                        let bars = parts[2];
                        let security = parts[3];

                        let is_secured = !security.is_empty() && security != "--";
                        let security_label = if is_secured { "Secured" } else { "Open" };
                        let display_string = format!("{:<25} Signal: {:>3}%  {}  {}", ssid, signal, bars, security_label);

                        if !discovered.iter().any(|n: &WifiNetwork| n.ssid == ssid) {
                            discovered.push(WifiNetwork {
                                display_string,
                                ssid,
                                is_secured,
                            });
                        }
                    }
                }
                let _ = tx.send(NetworkEvent::ScanComplete(discovered));
            } else {
                let _ = tx.send(NetworkEvent::ScanFailed);
            }
        });
    }

    /// Queries nmcli to ascertain if a local connection profile already exists.
    fn has_saved_profile(&self, ssid: &str) -> bool {
        let output = Command::new("nmcli")
            .args(["-t", "-f", "NAME", "connection", "show"])
            .stderr(Stdio::null())
            .output();

        if let Ok(out) = output {
            let saved_names = String::from_utf8_lossy(&out.stdout);
            return saved_names.lines().any(|line| line.trim() == ssid);
        }
        false
    }

    /// Spawns a thread attempting authentication against a pre-existing profile configuration.
    fn connect_saved_network(&mut self, ssid: String) {
        if self.is_busy { return; }
        self.is_busy = true;
        self.target_ssid = ssid.clone();
        
        self.log_message(&format!("Connecting to {}...", self.target_ssid));
        let tx = self.event_tx.clone();
        let thread_ssid = self.target_ssid.clone();

        std::thread::spawn(move || {
            let output = Command::new("nmcli")
                .args(["--wait", "10", "connection", "up", "id", &thread_ssid])
                .output();

            match output {
                Ok(out) if out.status.success() => {
                    let _ = tx.send(NetworkEvent::ConnectionSuccess(thread_ssid));
                }
                Ok(out) => {
                    let err_msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    let clean_err = if err_msg.is_empty() { "Unknown error".to_string() } else { err_msg };
                    let _ = tx.send(NetworkEvent::ConnectionFailed { ssid: thread_ssid, error: clean_err });
                }
                Err(e) => {
                    let _ = tx.send(NetworkEvent::ConnectionFailed { ssid: thread_ssid, error: e.to_string() });
                }
            }
        });
    }

    /// Handles network configuration/registration without blocking the UI thread.
    fn connect_new_network(&mut self, is_open_network: bool) {
        if !is_open_network && self.password_input.is_empty() {
            self.log_message("[ERROR] Password field cannot be empty for secured networks.");
            return;
        }

        if self.is_busy { 
            self.log_message("[WARN] Backend busy! Dropping connection request. Try again in a moment.");
            return; 
        }
        self.is_busy = true;

        let ssid = self.target_ssid.clone();
        let password = self.password_input.clone();
        
        self.password_input.clear();
        self.input_mode = InputMode::Normal;

        if is_open_network {
            self.log_message(&format!("Connecting to unsecure network {}...", ssid));
        } else {
            self.log_message(&format!("Connecting to {}...", ssid));
        }
        
        let tx = self.event_tx.clone();
        let already_has_profile = self.has_saved_profile(&ssid);

        // Spawn a background thread so the TUI stays responsive
        std::thread::spawn(move || {
            // Clear out the old profile completely to avoid bad system properties
            if already_has_profile {
                let _ = Command::new("nmcli")
                    .args(["connection", "delete", "id", &ssid])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .output();
            }

            // Build connect command using clean argument positioning
            let mut args = vec!["--wait", "10", "device", "wifi", "connect", &ssid];
            
            if !is_open_network {
                args.push("password");
                args.push(&password);
            }

            let output = Command::new("nmcli")
                .args(&args)
                .output();

            // Process output
            match output {
                Ok(out) if out.status.success() => {
                    let _ = tx.send(NetworkEvent::ConnectionSuccess(ssid));
                }
                Ok(out) => {
                    let err_msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    let _ = tx.send(NetworkEvent::ConnectionFailed { ssid, error: err_msg });
                }
                Err(e) => {
                    let _ = tx.send(NetworkEvent::ConnectionFailed { ssid, error: e.to_string() });
                }
            }
        });
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let original_panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_panic_hook(panic_info);
    }));

    let app = NetworkManagerApp::new();
    let run_result = run_application_loop(&mut terminal, app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = run_result {
        let log_path = "/tmp/awac_crash.log";
        if let Ok(mut file) = File::create(log_path) {
            let _ = writeln!(file, "[APPLICATION EXECUTION CRASH]: {:?}", err);
            println!("\x1b[31m[ERROR] The application crashed unexpectedly.\x1b[0m");
            println!("Details written safely to: {}", log_path);
        } else {
            eprintln!("[APPLICATION EXECUTION CRASH]: {:?}", err);
        }
    }

    Ok(())
}

fn run_application_loop<B: Backend>(terminal: &mut Terminal<B>, mut app: NetworkManagerApp) -> io::Result<()> {
    loop {
        while let Ok(event) = app.event_rx.try_recv() {
            app.is_busy = false; 
            match event {
                NetworkEvent::ConnectionSuccess(ssid) => {
                    app.log_message(&format!("[SUCCESS] Connected to {}.", ssid));
                }
                NetworkEvent::ConnectionFailed { ssid, error } => {
                    app.log_message(&format!("[FAIL] Could not connect to {}.", ssid));
                    app.log_message(" -> Error details written to /tmp/awac_errors.log");

                    let log_path = "/tmp/awac_errors.log";
                    if let Ok(mut file) = File::options().create(true).append(true).open(log_path) {
                        let _ = writeln!(file, "--- Connection Failure ---\nSSID: {}\nError: {}\n", ssid, error);
                    }
                }
                NetworkEvent::ScanComplete(networks) => {
                    app.networks = networks;
                    if !app.networks.is_empty() && app.list_state.selected().is_none() {
                        app.list_state.select(Some(0));
                    }
                    let network_count = app.networks.len();
                    app.log_message(&format!("Scan complete. Found {} networks.", network_count));
                }
                NetworkEvent::ScanFailed => {
                    app.log_message("[ERROR] Failed to query local NetworkManager backend interface.");
                }
            }
        }

        terminal.draw(|frame| draw_ui(frame, &mut app))?;

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Release {
                    continue;
                }

                match app.input_mode {
                    InputMode::Normal => match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Char('r') => app.scan_networks(true),
                        KeyCode::Up | KeyCode::Char('k') => {
                            if let Some(selected) = app.list_state.selected() {
                                if selected > 0 { app.list_state.select(Some(selected - 1)); }
                            }
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            if let Some(selected) = app.list_state.selected() {
                                if selected < app.networks.len() - 1 { app.list_state.select(Some(selected + 1)); }
                            }
                        }
                        KeyCode::Char('e') => {
                            if let Some(index) = app.list_state.selected() {
                                let target_network = &app.networks[index];
                                if target_network.is_secured {
                                    app.target_ssid = target_network.ssid.clone();
                                    app.password_input.clear();
                                    app.input_mode = InputMode::Editing;
                                    app.log_message(&format!("Updating password for {}...", app.target_ssid));
                                }
                            }
                        }
                        KeyCode::Enter => {
                            if let Some(index) = app.list_state.selected() {
                                let target_network = &app.networks[index];
                                let ssid = target_network.ssid.clone();
                                
                                if !ssid.is_empty() {
                                    if app.has_saved_profile(&ssid) {
                                        app.connect_saved_network(ssid);
                                    } else if !target_network.is_secured {
                                        app.target_ssid = ssid;
                                        app.connect_new_network(true);
                                    } else {
                                        app.target_ssid = ssid;
                                        app.input_mode = InputMode::Editing;
                                    }
                                }
                            }
                        }
                        _ => {}
                    },
                    InputMode::Editing => match key.code {
                        KeyCode::Enter => {
                            app.connect_new_network(false);
                        }
                        KeyCode::Char(c) => {
                            app.password_input.push(c);
                        }
                        KeyCode::Backspace => {
                            app.password_input.pop();
                        }
                        KeyCode::Esc => {
                            app.input_mode = InputMode::Normal;
                            app.password_input.clear();
                            app.log_message("Password entry cancelled.");
                        }
                        _ => {}
                    },
                }
            }
        }
    }
}

fn draw_ui(frame: &mut ratatui::Frame, app: &mut NetworkManagerApp) {
    let main_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(65),
            Constraint::Length(3), 
            Constraint::Min(5),
        ])
        .split(frame.size());

    let list_widget = List::new(
        app.networks
            .iter()
            .map(|network| ListItem::new(network.display_string.as_str()).style(Style::default().fg(Color::White)))
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Available Networks ")
            .title_style(Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD))
            .border_style(Style::default().fg(COLOR_PRIMARY)),
    )
    .highlight_style(
        Style::default()
            .bg(COLOR_BG_DARK)
            .fg(COLOR_SECONDARY)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("▶ ");

    frame.render_stateful_widget(list_widget, main_layout[0], &mut app.list_state);

    let help_text = match app.input_mode {
        InputMode::Normal => {
            " [q] Quit  [r] Rescan  [▲/▼ or j/k] Navigate  [Enter] Connect  [e] Enter New Password "
        }
        InputMode::Editing => {
            " [Esc] Cancel  [Backspace] Delete  [Enter] Submit "
        }
    };

    let help_widget = Paragraph::new(help_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Controls ")
                .title_style(Style::default().fg(COLOR_PRIMARY).add_modifier(Modifier::BOLD))
                .border_style(Style::default().fg(COLOR_SECONDARY)),
        )
        .style(Style::default().fg(Color::White));

    frame.render_widget(help_widget, main_layout[1]);

    let layout_log_inner_height = main_layout[2].height.saturating_sub(2);
    let total_log_lines = app.logs.len() as u16;

    let vertical_scroll_offset = if total_log_lines > layout_log_inner_height {
        total_log_lines.saturating_sub(layout_log_inner_height)
    } else {
        0
    };

    let constructed_log_lines: Vec<ratatui::text::Line> = app
        .logs
        .iter()
        .map(|log_entry| ratatui::text::Line::from(log_entry.as_str()).style(Style::default().fg(COLOR_ACCENT)))
        .collect();

    let logs_widget = Paragraph::new(constructed_log_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Console ")
                .title_style(Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD))
                .border_style(Style::default().fg(COLOR_PRIMARY)),
        )
        .scroll((vertical_scroll_offset, 0));

    frame.render_widget(logs_widget, main_layout[2]);

    if let InputMode::Editing = app.input_mode {
        let overlay_vertical_split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(35),
                Constraint::Length(5), 
                Constraint::Percentage(45),
            ])
            .split(frame.size());

        let centered_popup_area = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(20),
                Constraint::Percentage(60),
                Constraint::Percentage(20),
            ])
            .split(overlay_vertical_split[1])[1];

        let secure_password_mask = "*".repeat(app.password_input.len());

        let password_modal_widget = Paragraph::new(format!("\n Password: {}", secure_password_mask))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {} ", app.target_ssid))
                    .title_style(Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD))
                    .border_style(Style::default().fg(COLOR_PRIMARY)),
            )
            .style(Style::default().fg(Color::White));

        frame.render_widget(Clear, centered_popup_area); 
        frame.render_widget(password_modal_widget, centered_popup_area);
    }
}
