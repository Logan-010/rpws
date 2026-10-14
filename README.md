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

## Firefox peer sync

Build the extension as described in `firefox-extension/README.md`, and rebuild
and restart the daemon (`cargo build -p rpws`). Import the base64 `secret` from
the daemon configuration into extension setup. Both clients derive the same
encryption key from that 32-byte seed.

Use a WebSocket listener as the extension's bootstrap address, such as
`/ip4/127.0.0.1/tcp/7776/ws/p2p/<daemon-peer-id>`. The HTTP API and plain TCP/QUIC
listeners cannot serve browser libp2p connections.

Peers exchange authenticated encrypted password mappings and payloads over
`/rpws/sync/1.0.0` on connection and every five seconds. The extension's
**Refresh** waits for a network sync, caches the results, and reports connection
or secret mismatch errors. This lets a new peer discover preexisting passwords
without already knowing their names. Older daemons need to be rebuilt to expose
this enumeration protocol.
