# AWAC (Aggressive Wi-Fi Access Client)
AWAC is a lightweight, responsive Terminal User Interface (TUI) wrapper for nmcli (NetworkManager). The intent is for this to be run on direct TTYs on Raspberry Pi powered devices such as the Clockwork uConsole. AWAC provides a keyboard-driven interface to rapidly scan, authenticate, and manage local Wi-Fi connections without dealing with nmcli commands.

## Features
Tailored for Direct TTYs: Designed to render perfectly on small hardware displays and standard virtual consoles without relying on xterm features or desktop server environments.

Thread-Isolated Networking: Network scanning and authentication commands run completely in background threads, keeping the interface fluid and responsive while operations execute.

Safe Terminal Restoration: Includes custom panic hooks to ensure your terminal's raw mode is gracefully restored if the application ever encounters an unexpected error—preventing broken or frozen TTY lines.

Smart Profile Detection: Automatically checks for pre-existing NetworkManager connection profiles. If a profile exists, it connects instantly; if not, it prompts you for credentials securely via an on-screen overlay modal.

Live Console Logging: An integrated, rolling log viewer at the bottom of the interface tracks connection states and backend execution in real-time.

## Prerequisites
Because AWAC acts as a frontend interface, it requires a Linux environment running NetworkManager.

nmcli installed and running as your active network backend.

Valid permissions to manage network interfaces (you may need to run the compiled binary with sudo or configure appropriate Polkit rules depending on your system setup).

## Installation and Setup
Clone the repo:
`git clone https://github.com/jason1595/awac.git`

Go to the download loaction:
`cd awac`

Build the release binary:
`cargo build --release`

Make the binary executable:
`sudo chmod +x ./target/release/awac`

Run it:
`./target/release/awac`

In order to make this run reliably without polluting the inreface I place the binary in my home folder and wrap it in a bash function:
```bash
awac() {
    clear
    ~/awac
    clear
}
```

## Controls
AWAC utilizes simple, deterministic keyboard navigation shortcuts:

| Key | Action |
| :--- | :--- |
| `▲` / `▼` or `k` / `j` | Navigate up and down the discovered access point list |
| `Enter` | Connect to the selected network (or open the password modal) |
| `e` | Explicitly enter or overwrite a security password |
| `r` | Force a hardware Wi-Fi rescan |
| `Esc` | Cancel an in-progress password entry |
| `q` | Quit AWAC cleanly |

## Diagnostics and Logs

If a connection fails or the application encounters an error, details are written away from the main interface to ensure the small layout remains clean. You can inspect the diagnostic artifacts here:

* **Connection Failures:** /tmp/awac_errors.log`
* **Unexpected Application Crashes:** `/tmp/awac_crash.log`
