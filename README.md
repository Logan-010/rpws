# rpws - Rust PaWS

Reliable P2P password manager.

## Terminal UI

Start the daemon with `rpws daemon`, then run `rpws` in another terminal to open
the vault. Use `--api-addr` to connect to a different daemon.

The list expands the selected entry, showing its username, URL, notes, and last
update time. Datetimes use your local timezone, displayed as
`2026-10-13 14:30:00 EDT`. Passwords stay masked until you reveal them.

| Key | Action |
| --- | --- |
| ↑ / ↓ or j / k | Select an entry |
| Enter / Space | Expand or collapse details |
| v | Reveal or hide the selected password |
| c | Open the clipboard field picker |
| a / e | Add / edit a password |
| d / Delete | Remove a password (with confirmation) |
| r | Refresh the list |
| q / Esc | Quit |

In the editor, use Tab / Shift+Tab to switch fields, arrow keys to move the
cursor, Ctrl+S to save, and Esc to cancel. Entry names are fixed when editing.
Failed saves preserve your input so you can retry. Network requests run in the
background, and Ctrl+C exits from any screen.

Press `c` on an entry to choose its name, URL, username, password, notes, or
formatted update time. Use ↑ / ↓ (or j / k) to select a field, Enter to copy it,
and Esc to cancel. Copying preserves the original text, including multiline
notes, and does not reveal passwords on screen. Clipboard failures appear in
the status line, leaving the picker open to retry. Clipboard access supports
X11 and Wayland (with a data-control-capable compositor) on Linux.

Removal deletes the local name mapping and stops advertising it. It does not
delete independent copies stored on remote peers.
