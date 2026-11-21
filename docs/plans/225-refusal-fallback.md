# #225: Model refusals fall back and show as themselves

For Mike's review before building. Backend: Sam. Verified against the code and
Anthropic's current docs (platform.claude.com/docs/en/build-with-claude/refusals-and-fallback).

## Today, in the code

- Anthropic's classifier models (Fable 5.x, Opus 5.5/5, Sonnet 5.5, Haiku 5.5)
  can decline a request as a normal response: `stop_reason: "refusal"`,
  `stop_details` carrying `category` (cyber, bio, frontier_llm,
  reasoning_extraction, general_harms, or null), `explanation`, and (only with
  the fallbacks parameter set) `recommended_model`.
- Our provider maps `"refusal"` to `StopReason::Error` with the explanation as
  the error message and drops the category
  (anthropic/mod.rs `map_stop_reason`). The run ends, and the transcript shows
  a plain red Notice — the refusal is indistinguishable from a network error.
- We already send Anthropic's **server-side fallback** (beta
  `server-side-fallback-2026-07-01`, the `fallbacks` array) for models whose
  catalog entry names `allowedFallbackModels`: Fable 5 → Opus 4.8/Opus 5,
  Opus 5 → Opus 4.8, Opus 5.5 → Opus 5. The parser already reprices when the
  served model differs. So for those models, most refusals are already retried
  inside the API call — silently: the person never learns another model
  answered.

## What Anthropic's side covers, and what it can't

Server-side fallback exists exactly for refusals ("Only a safety classifier
decline triggers the fallback"), retries in one round trip, and applies
fallback credit so the fallback attempt's prompt-cache miss isn't paid twice.
It does NOT cover:

1. Models without it (Haiku 5.5; anything non-Anthropic later).
2. Categories with no permitted fallback (reasoning_extraction never falls
   back; a category can lack a recommendation).
3. A rate-limited/overloaded fallback model: the refusal is returned with
   `stop_details.recommended_model` as a hint.
4. Sonnet 5.5, today, in our catalog: it has classifiers but we name no
   fallbacks for it, so we don't send the parameter at all.

So: keep the server side as the first line and widen it; build a small client
side for what falls through; and make every refusal and every handoff visible.

## Design

**1. Widen the server side (anthropic provider + catalog, no protocol).**
Keep named `allowedFallbackModels` lists where the catalog has them. Add a
catalog compat flag `serverSideFallback: true` on the classifier models that
support it (Fable 5.x, Opus 5.5/5, Sonnet 5.5 — not Haiku 5.5); when it's set
and no list is named, send `fallbacks: "default"`, which uses Anthropic's
per-category recommendation. One response either way; the served model is in
the response and already repriced.

**2. The refusal becomes itself (types + transcript + protocol).**
New `StopReason::Refusal`, with the category, explanation and
recommended_model kept on the message (structured, in `extra`, like the
account marks). Everywhere `StopReason::Error` is handled, Refusal joins the
same arm (run ends, excluded from estimates/compaction) except the transcript:
the Notice gains a `refusal_category` field (number from you), so the app can
show "Claude declined this request (cyber): <explanation>" as its own card,
not a generic failure. Server-side handoffs become visible too: when the
served model differs from the requested one, push an info Notice ("Fable 5
declined this request (cyber); Opus 5 answered instead."), since today the
person has no way to know another model answered (the app shows the real
state).

**3. Client-side fallback for what falls through (agent.rs + config).**
Config: `fallback_model` per agent and `default_fallback_model` in settings
(JSON like `model`, same validation; agent's wins; empty = none). On a
Refusal, when a fallback model is configured, differs from the model that
refused, and this run has retried fewer than 2 refusals: re-send the same
context on the fallback, that turn only; the next turn goes back to the
agent's own model. The agent is told in a Claktory event what happened
("<model> declined this step (cyber); the answer came from <fallback>" / "…no
fallback model is set, so the step ended; rephrase or carry on"). If the
fallback account pool has no capacity, the refusal stands at once — never
wait out limits for a retry (mirrors Anthropic's own rule). If the fallback
also refuses, the refusal stands (counts toward the cap of 2 per run).

**One retry per turn, not stay-on-alternate.** Staying on the alternate until
the next context would silently keep the agent on a model the person didn't
pick, and the primary's warm cache is lost for the whole remainder. One retry
costs one cache write on the fallback per retried turn; across close retries
the fallback's own cache is warm too (TTL), and the per-run cap of 2 bounds
the worst case (a conversation whose content itself trips the classifier every
turn). When building, read Anthropic's fallback-credit page: a manual retry
can claim the credit, which refunds the fallback's cache write; if it applies
to our client-side retry, the cost argument gets even easier.

**4. Pools.** The client-side retry resolves accounts for the fallback model
exactly as a normal request for that model would: a Fable → non-Fable
fallback draws from the shared pool, with the existing per-pool
`exhausted_until` logic. No new pool state.

## Protocol needs (numbers from you)

- `Notice.refusal_category` (string; empty = not a refusal). The card is app
  work (Zoe/Priya to own; the wording through Hana).
- `Agent.fallback_model` in Workspace + `UpdateAgent.fallback_model` +
  `UpdateSettings.default_fallback_model` (all optional strings), so the app
  can offer it in the agent's settings and Settings → Defaults; config keys
  `fallback_model` / `default_fallback_model` beside `model` / `default_model`.
- No new CM/SM.

## Out of scope

Cross-provider fallback (OpenAI's refusal shape differs; no credit), the
Batches API (we don't batch), SDK middleware (we speak raw HTTP), and any
automatic model change that lasts beyond the refused turn.

## Billing notes (for the manual later)

A refusal before any output is billed only in bio/frontier_llm/
reasoning_extraction; mid-stream refusals bill what streamed. With fallback,
the refusal that triggered it may be billed in addition to the fallback
request. Usage already reprices by the served model.
