# Calendar, a Claktory app

Everything time-based in one place: what your agents have scheduled, what
they're running and what they've planned. It's one of Claktory's apps: a
WebAssembly component of the `claktory:app@1.1` world `app-with-views`,
run by Claktory's app host, which gives it only what its manifest asks for
and you approve.

## How it works

- **Data:** it asks to read Schedule (`[[data]]` kind `schedule`:
  monitors' names and at, every and cron schedules, never a check's or a
  watch's command; checklist items' text and dates; agents' names and
  projects) and Activity (times and states alone). With Activity alone,
  monitors show untitled ("Fugo's monitor") and items, having no dates there,
  don't show. Until one is granted, its view says how, and it reads
  nothing.
- **Derived entries,** read live at every look and never kept
  (`src/layers.rs`): monitors' next fires (every and cron expanded on the
  Mac's clock; more than 4 a day, or a check, files, watch or pipes
  monitor, is one all-day band), what they fired, blocked and parked
  items' dates and when the person is asked again, background tasks of 5
  minutes or more or running, and runs over 30 minutes.
- **Placed events,** kept in its store (`src/store.rs`): `events` (an
  index of each event's span), `ev/<id>`, `prev/<id>` (the version before
  its last change, or its deletion), `calendars`, `prefs` and
  `device/<stand-in>` (the view each device chose last). Each has a
  `_rev`; repeats (every day, week or month, on weekdays, until a time or
  a count) keep the Mac's wall-clock time. Events whose last occurrence
  ended over 400 days ago are dropped once a day.
- **Time:** times are UTC milliseconds. The Mac's clock comes from
  SQLite's `localtime` in factoryd, sampled each hour (`src/time.rs`), so
  cron schedules and repeats run as Claktory's monitors run them, across
  daylight saving. Devices draw in their own zone; all-day dates go to
  them as UTC midnights, read as dates, end exclusive.
- **The view:** `main`, Claktory's Calendar component (day, week and
  agenda) with the Agent, Show and ⋯ menus it places in its bar
  (`src/view.rs`). Each move in it (a swipe, ‹ ›, Today, the view switch)
  comes back as the `calendar` action; the app keeps the anchor and the
  view and sends the rows of 8 days each side. While a view is open it
  looks again every minute and pushes only what changed.
- **Tools,** by scope (`src/tools.rs`): `view` gives `calendar_list` and
  `calendar_get`; `edit` also `calendar_put`, `calendar_move`,
  `calendar_delete`, `calendar_calendars` and `calendar_prefs`. An agent
  sees and changes its own project's events and entries, an admin all;
  what an agent that has read the web or a pipe places is marked as from
  outside. When someone else moves or deletes an event its maker linked to
  its monitor, the maker is told (event `moved`).

## Building

```sh
rustup target add wasm32-wasip2
cargo test                                   # the logic, natively, against a stand-in host
cargo build --target wasm32-wasip2 --release # target/wasm32-wasip2/release/claktory_calendar.wasm
```

`./build.sh` builds it and packs `dist/calendar.clapp`, signed as Charts'
`build.sh` signs it (its header says how).
