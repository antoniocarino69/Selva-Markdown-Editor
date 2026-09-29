# Selva — Notes and connections

Selva (from Italian "forest") is a blazing fast, local-first Markdown note-taking app written entirely in Rust. Designed to be lightweight and resource-efficient (sub-100MB RAM footprint), Selva focuses on providing a seamless writing experience without the overhead of web-based technologies (no Electron/Chromium).

## Features

- **Nested Folders**: Create a folder from the vault menu, or right-click any folder and choose "New folder here…" to create a subfolder.
- **Clipboard Shortcuts**: Use Ctrl+C, Ctrl+X and Ctrl+V in the editor. The unreliable clipboard context menu has been removed.
- **Blazing Fast & Lightweight**: Written in Rust using the `egui` framework. Consumes very little RAM and launches instantly.
- **Obsidian Compatibility**: Fully compatible with your existing Obsidian vaults. It reads your local `.md` files directly and respects your folder structures.
- **Image Support**: Copy/Paste images directly into the editor (using `Ctrl+V` or `Cmd+V`). Images are saved locally to an `_assets` directory and automatically linked using wiki-link syntax (`![[image.png]]`).
- **Split View & Preview**: Toggle seamlessly between "View Only", "Edit Only", or a beautiful side-by-side "Split" mode.
- **Auto-pairing**: Smart character pairing for brackets and Markdown formatting characters (e.g. `*`, `[`, `(`, `"`).
- **Keyboard Shortcuts**: Native shortcuts for creating notes, saving, formatting, and copying text.
- **Cross Platform**: Works natively on Windows, macOS, and Linux.

## Why Selva?

While tools like Obsidian and Notion are incredibly powerful, they are built on top of web technologies (Electron/WebView) which can consume significant amounts of memory and CPU cycles. Selva was built from the ground up as a native desktop application to provide a pure, snappy, and distraction-free writing environment.

## Upcoming Features
- [ ] File System watcher (auto-refresh tree on external modifications)
- [ ] Cloud sync via remote CouchDB implementation
- [ ] Dark/Light mode toggle enhancements

## Building from source

Ensure you have [Rust and Cargo](https://rustup.rs/) installed.

```bash
git clone https://github.com/your-username/selva.git
cd selva
cargo build --release
```

The compiled binary will be available at `target/release/selva`.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
