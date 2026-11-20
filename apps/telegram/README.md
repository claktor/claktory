# Telegram, a Claktory app

A Telegram bot that's yours alone. You message it, and the agents you've
subscribed get your message as an event. Agents you've granted it can
message you there with `telegram_send`. It's one of Claktory's apps: a
WebAssembly component of the `claktory:app@1` world, run by Claktory's app
host, which gives it only what its manifest asks for and you approve.

## How it works

- **Setup:** the app's one secret is the bot token, from
  [@BotFather](https://t.me/BotFather) (`/newbot`). Claktory keeps it in the
  vault, granted to the app, never to agents.
- **Linking:** started, the app makes a one-time code and says in
  Settings → Apps how to link your chat: open `https://t.me/<bot>?start=<code>`
  and tap Start, or send the bot `/start <code>`. From then on it answers
  only that chat, and ignores everyone else.
- **Your messages:** it long-polls Telegram (`getUpdates`, 25 s at a time,
  started with `wasi:http` and looked at every second, so tool calls never
  wait behind it). Each message becomes a `message` event in scope `read`:
  `Matt: Can you check the deploy?`. A photo, file or the like says what it
  is, with its caption.
- **Tools:**
  - `telegram_send` (scope `send`): Markdown becomes Telegram's HTML; a
    long message goes in pieces.
  - `telegram_history` (scope `read`): the last messages both ways (200
    kept).
- **State:** everything lasting is in the app's own key-value store: the link,
  the code, the poll's offset, the bot's name and the history. A restart
  picks up where it was.

## Building

```sh
rustup target add wasm32-wasip2
cargo test                                   # the logic, natively, against a stand-in host
cargo build --target wasm32-wasip2 --release # target/wasm32-wasip2/release/claktory_telegram.wasm
```

`./build.sh` builds it and packs `dist/telegram.clapp` (manifest.toml,
app.wasm and signature, in an uncompressed tar), signed with a development
key it makes at `~/.claktory/app-dev-key.pem` on first use. The key stays
outside the repository; Claktory installs an app signed with it as tier
Local, and pins the key to the app at its first install.

`src/lib.rs` is the app, written against the `Host` trait; `src/tests.rs`
tests it with a stand-in host and Telegram. `src/wasm.rs` is the real
host, through the host's world (`crates/claktory-apps/wit/app.wit`) and `wasi:http`. `manifest.toml` is what
Claktory shows for approval when you install it: the host it reaches
(api.telegram.org), its secret, scopes, events and tools.
