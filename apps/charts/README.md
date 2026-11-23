# Charts, a Claktory app

Live charts of what your agents do, from the data you let it read. It's
one of Claktory's apps: a WebAssembly component of the `claktory:app@1.1`
world `app-with-views`, run by Claktory's app host, which gives it only
what its manifest asks for and you approve.

## How it works

- **Data:** it asks to read your agents' activity (`[[data]]` kind
  `activity`: runs, usage, tasks, monitors, checklists and messages' times,
  never their text) and, one by one as you choose, agents' own databases
  (`agent-database`, `agent = "*"`), and CSV or TSV files you choose
  (`file`: each one table, `rows`, read again at every query). Nothing is
  shared by default: Rivet sets up what it reads and which agents may change
  it when you ask (a card asks you first for anything private, such as an
  agent's database or a file), or you turn a source on in Marketplace →
  Apps; you can turn it off at any time. Until one is on, its view says how, and it reads nothing. A
  Claktory that gave it all of Claktory's records (`factory-records`)
  before keeps that until you turn it off.
- **Dashboards:** definitions kept in the app's own key-value store
  (`dashboards`, the list; `dashboard/<id>`; `previous/<id>`, the version
  before the current one). Each has a name, the range it opens on
  (`time`), how often it refreshes (`refresh_s`, 10 seconds to a day), up to
  4 variables and up to 24 panels. A panel is one read-only SQL query against
  a source, where it sits on a 12-column grid (`grid`: x, y, w, h) and how
  its answer shows (`viz`): `stat` (the last column's total, its rows as a
  spark line), a chart (`timeseries`, `bars`, `stackedBars`, `pie`,
  `scatter`) or `table`. `src/model.rs` has the schema and its checks, and
  the manifest's `instructions` tell agents.
- **Bound values:** `$from`, `$to` (milliseconds; `$from_s`, `$to_s` in
  seconds), `$now`, `$step` (a bucket that makes at most about 100 of the
  range) and each variable (`$agent`) are bound to the query as values
  (`src/sql.rs`), never spliced into it.
- **Revisions:** every save counts `_rev` up and keeps the version it
  replaces; Restore previous version (or `charts_get` with `previous`) puts
  it back. An agent's put says which `_rev` it read and is refused if the
  dashboard changed since.
- **The default dashboard,** made on first start: runs, messages, checklist
  items done and tokens (input and output) over the last 7 days, tokens and
  runs by agent, and the background tasks running now, with an Agent
  variable.
- **The view:** `main`, drawn by Claktory's own components (catalog v1).
  The person picks the dashboard, the range and each variable's value, and
  under Edit adds, changes and removes panels in an editor whose Run shows
  the query's answer as the panel would before Save, renames, makes and
  deletes dashboards, and restores the previous version. While someone has
  it open, the app runs the queries every `refresh_s` seconds and pushes
  only what changed; once the last view closes, nothing runs. Long series
  are downsampled to 2,000 rows, tables cut at 200.
- **Tools,** by scope: `view` gives `charts_list` and `charts_get`; `edit`
  also `charts_put`, `charts_delete` and `charts_query` (200 rows, bound as a
  panel's, with a dashboard's variables when it names one), and with them
  reading anything Charts can read.

## Building

```sh
rustup target add wasm32-wasip2
cargo test                                   # the logic, natively, against a stand-in host
cargo build --target wasm32-wasip2 --release # target/wasm32-wasip2/release/claktory_charts.wasm
```

`./build.sh` builds it and packs `dist/charts.clapp` (manifest.toml,
app.wasm and signature, in an uncompressed tar), signed with a development
key it makes at `~/.claktory/app-dev-key.pem` on first use. The key stays
outside the repository; Claktory installs an app signed with it as tier
Local, and pins the key to the app at its first install.

`src/lib.rs` is the app, written against the `Host` trait, with
`model.rs` (dashboards and the store), `sql.rs` (binding), `view.rs` (what a
view shows) and `tools.rs` (agents' tools); `src/tests.rs` tests it with a
stand-in host whose queries run in SQLite. `src/wasm.rs` is the real host,
through the host's world (`crates/claktory-apps/wit/app.wit`).
