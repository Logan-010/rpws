# RPWS Firefox extension

A Firefox popup styled with Tailwind CSS, backed by the Rust WASM API in
`../web`. A persistent background page owns the WASM instance and peer connections.

## Build and load

Prerequisites: Node.js 20.19+ (or 22.12+), npm, Cargo, and the Rust
`wasm32-unknown-unknown` target. The repository's Nix development shell provides
Node and the Rust target. Outside that shell, install the target with
`rustup target add wasm32-unknown-unknown`.

```sh
npm install
npm run build
npm run lint
npm test
```

`build` uses the locally installed `wasm-pack` to compile `../web`, generates
bindings in `wasm/`, then bundles the extension and Tailwind CSS into `dist/`.
No runtime assets are fetched from a CDN.
The build skips wasm-pack's legacy optional `wasm-opt` pass for compatibility
with the repository's current Rust WASM features; Cargo still builds in release mode.

In Firefox, open `about:debugging#/runtime/this-firefox`, choose **Load Temporary
Add-on**, and select `dist/manifest.json`. Open **RPWS** from the toolbar.
Alternatively, run `npm start` to launch a development Firefox instance.
Run `npm run package` to create an archive for distribution/signing.
After UI-only edits, `npm run build:ui` reuses the generated WASM bindings.

## Usage

- First open: choose **Generate a new secret** (the default) or **Import an
  existing secret (base64)**. Imported secrets must decode to exactly 32 bytes;
  standard base64 with or without the final `=` padding is accepted. Generation
  uses the WASM API's random secret. Then enter one bootstrap multiaddress per
  line and select **Create vault**.
  Browser peers must support WebSockets and addresses must end in a peer ID,
  for example `/dns4/peer.example/tcp/443/wss/p2p/<actual-peer-id>`.
- Subsequent opens load the save from `browser.storage.local` automatically.
- Closing and reopening the popup restores its screen, search, expanded entries,
  scroll position, and unfinished setup/password/settings forms. UI state is held
  in the persistent background page for the current browser session. Passwords
  are masked again on reopen. Creating, importing, or unloading a save clears
  the previous vault's UI state and drafts.
- Add, search, edit, reveal, copy, or remove passwords. Names remain fixed while
  editing. Removal affects local mappings; independent remote copies remain.
- **Open** opens an entry's HTTP/HTTPS URL in a new tab. Entries without a URL
  have this button disabled.
- **Autofill** appears on logins whose saved URL has the same exact origin as
  the active tab (scheme, hostname, and port). Matching entries appear first when
  search is empty. Open a site's login form, open RPWS, and choose **Autofill**.
  Set the entry's URL to the actual login site's origin if it differs from the
  site's homepage. Passwords are never filled on subdomains or HTTP downgrades
  of a saved HTTPS URL.
- **Ctrl+Shift+L** (**Command+Shift+L** on Mac) fills the current page when there
  is exactly one matching login. With zero or multiple matches it opens the popup
  so you can choose a login. Customize the shortcut in Firefox's **Manage
  Extension Shortcuts** screen if it conflicts with another extension.
- Autofill dispatches input/change events for interactive forms, supports forms
  added dynamically and open shadow roots, and avoids hidden, disabled,
  read-only, and new-password fields. Focus a field to choose between login forms.
  For username-first sign-ins, fill the username, advance, then autofill again
  on the password step. Forms are never submitted automatically. Autofill targets
  the top-level page; login forms inside iframes are not filled.
- **Copy** opens a modal field picker for the name, URL, username, password,
  notes, or formatted update time. Use Tab or arrow keys to choose a field,
  Enter to copy, and Escape or Cancel to close. Passwords stay masked in the
  picker; copying preserves the original field text, including multiline notes.
- The search bar fuzzy-matches names, usernames, URLs, and notes, tolerates
  small typos, and ranks the best matches first. Password contents are excluded
  from search. Search text and an open copy picker survive popup reopening.
- **Settings** lets you add, remove, or modify bootstrap addresses by editing
  the list. Saving it restarts the WASM API with the updated peers.
- **Settings → Copy secret as base64** copies the current vault's secret for
  setting up another peer. Importing just a secret creates a fresh peer identity
  and an empty local record cache; import a complete save to restore cached
  passwords and identity as well.
- **Export save** downloads the native binary save, including the identity,
  vault secret, addresses, and cached encrypted records. **Import save** validates
  and replaces the current save after confirmation. These files use the native
  `web::Save` format, not a separate extension-specific JSON format.
- **Settings → Unload save** opens a confirmation warning that unsaved data will
  be lost. Confirming clears the cached save from this Firefox profile, stops
  the active WASM session, and returns to setup. You can export a backup directly
  from the confirmation screen or cancel to keep the current vault.

The background page snapshots saves after each mutation, before export, and
every 30 seconds for peer-received records. Firefox storage is local to the
extension/profile; temporary add-on storage should not be treated as a backup.
Save files contain the secret needed to decrypt records: keep them private.
There is no master-password/unlock layer in the existing WASM API.

The browser and daemon derive the same encryption key from the 32-byte secret
seed. Import the base64 `secret` from the daemon's config when setting up the
extension. Existing browser saves using the old direct-key format are migrated
when loaded, preserving their passwords and the original seed.

Peers enumerate authenticated encrypted mappings and payloads using
`/rpws/sync/1.0.0`. Sync runs on connection and periodically, and **Refresh**
waits for a network sync before rereading and saving the cache. Both the daemon
and extension must be rebuilt from this version. Use the daemon's WebSocket
listener (by default `/ip4/127.0.0.1/tcp/7776/ws/p2p/<daemon-peer-id>`), not its
TCP-only or HTTP API address. Refresh reports unreachable peers, unsupported
sync protocols, and mismatched secrets instead of claiming a cache-only refresh.
The extension requests host access so the WASM WebSocket transport can contact
user-configured bootstrap peers and autofill can reach login pages. The autofill
content script is injected only on request and receives only the selected login's
username and password after the background page checks the active site's origin.
Firefox-restricted pages cannot be autofilled.

## Verification

`npm test` exercises the actual compiled WASM offline: save validation, bootstrap
addresses, add/edit/delete, and restoring password records from save snapshots.
It uses Node timer shims and does not test Firefox permissions or peer networking.
It also exercises the background message handler with the real WASM and mocked
Firefox storage, including invalid imports and changing peers without losing records.
DOM-based popup tests close and recreate the popup to check draft/view restoration,
pending imports, and resetting UI state after switching or unloading a save.
Autofill tests cover site matching, credential isolation, shortcut selection,
form events, username-first steps, shadow roots, and rejecting unavailable or
signup/reset fields.

After `npm run build:wasm`, run the live local WebSocket integration test from
the repository root:

```sh
cargo test -p rpws browser_sync_enumerates_daemon_passwords_over_websocket -- --ignored --nocapture
```

This starts a native daemon swarm with preexisting passwords and connects the
actual compiled WASM using Node's WebSocket implementation. It checks unknown
name discovery, cache restoration, browser-to-daemon writes, and secret isolation.

After building and linting, check in Firefox:

1. Create a vault with a valid WebSocket multiaddress; close and reopen the popup.
2. Add an entry containing quotes or markup in its fields, reveal/copy it, edit
   it, and verify the fields survive popup reopening and browser restarting.
3. Export a save, make a change, and import the exported save to restore it.
4. Try an invalid save or address; the existing vault should remain available.
5. Modify bootstrap peers in Settings, reopen, and verify the saved addresses.
6. Remove an entry and confirm it stays absent after reopening.
7. Choose **Unload save**, cancel once, then export and confirm. Reopen the popup
   to verify setup appears, and import the backup to restore the vault.
8. Open a matching site's login page and choose **Autofill**. Check username and
   password fill without submission, including a username-first flow. Verify the
   shortcut with one matching login and the popup chooser with multiple logins.
9. Open a different host or HTTP version of a saved HTTPS site and verify that
   the saved login has no Autofill button.
