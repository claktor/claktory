# #223: Secret requests become approvals, and Rivet does the vault work

Design for Mike's review; nothing here is built yet. Today's flow asks the
person to do the matching themselves: an agent's `secrets_request` names
variables, and the card in its conversation offers "give an existing
secret" or "add a new one". Picking an existing secret whose variable
names differ produces the alias machinery and its wording, which confused
Matt. The redesign: the person approves the *need*; the vault work —
finding a matching secret or collecting a new one — moves to an admin
agent (Rivet), the only kind that can see the vault's names.

## The flow

1. **The agent asks for a kind of secret.** The `secrets_request` tool is
   unchanged: `name`, `description`, `variables`, `reason`. What changes
   is the instruction text: the description must say what the secret has
   to be *able to do*, and roughly how much ("an Anthropic API key that
   can run Claude models a few times"), not just which service. The
   variables stay as the technical shape of what the agent's commands
   expect.

2. **The person approves the need.** The card (agent's conversation,
   Needs you, top of Settings → Secrets) states it plainly: who needs
   what, what it must be able to do, why — the variables as fine print.
   Its actions: **Approve**, **Dismiss** (with an optional comment), and
   a quiet **Choose a secret…** that opens today's full dialog for
   someone who wants to do it directly. No variable matching on this
   card, ever.

3. **Approval passes it to the admin agent.** The request moves to a new
   state, APPROVED, and the chosen admin agent gets an event written to
   stand alone: the asker, the name, the description, the variables, the
   reason, and the person's comment, plus what to do — `secrets_list` to
   look, `secrets_grant` with `request` to give a matching secret, or
   `secrets_collect` to ask the person to add one. The admin agent:
   - **grants an existing secret**: `secrets_grant` gains an optional
     `request` (and, with it, optional `aliases` and `as_is`): the grant
     goes to the *asker*, the existing alias logic applies (auto-pairing
     one missing name with one unused variable; any other mismatch is
     refused back to the admin agent, which passes `aliases` explicitly —
     the person never sees this), the request becomes GRANTED, and the
     asker is told exactly as today.
   - **or collects a new one**: `secrets_collect` (`request`) shows a
     card in the admin agent's conversation (and Needs you, and Settings
     → Secrets), the add-secret dialog prefilled with the request's name,
     description and variables. The person types the values on whatever
     device they're using; they go straight into the vault, never through
     the agent — the same promise as `devices_add`, which this card
     copies. Saving creates the secret, grants it to the asker, marks the
     request GRANTED, and tells both agents how it ended. One collect
     card per request; the admin agent or the person can cancel it.

4. **The asker is told what it got**, exactly as today (the decision
   event in `decide_request` is untouched).

## Protocol deltas

No new ClientMessage or ServerMessage: the collect card saves through the
existing `DecideSecretRequest` (its add-new path), bound to the original
request. Only fields and one enum value are added; nothing renumbered.

- `SecretRequest`:
  - `13: int64 approved_ms` — when the person approved the need; 0
    otherwise.
  - `14: string admin_agent_id` — the admin agent the approval passes it
    to, set when the request is made (and shown so the app knows which
    card to draw); empty when no admin agent with Secrets exists, and the
    app then shows today's full card.
  - `15: bool collecting` — an add-secret card for this request is open
    in the admin agent's conversation.
- `SecretRequestState`: `6: SECRET_REQUEST_STATE_APPROVED`.
- `DecideSecretRequest`: `12: bool approve_only` — with `approve`,
  approve the need and pass it to `admin_agent_id` instead of giving
  anything. Refused when the request has no admin agent (the app never
  offers it then). All existing fields keep their meaning, so an older
  app's direct give keeps working against a newer backend, and vice
  versa.

## States

- PENDING → APPROVED (new), or GRANTED/DISMISSED directly (the person
  used Choose a secret…, or there's no admin agent), or
  EXPIRED/WITHDRAWN as today.
- APPROVED → GRANTED (the admin agent granted or collected), DISMISSED
  (the person dismisses it after all, from the card), or WITHDRAWN (the
  asker cancels; an open collect card closes).
- The 12-hour expiry applies only while PENDING. An APPROVED request
  doesn't expire: it's the person's own standing decision, its card stays
  in Needs you with "Passed to Rivet" and the direct actions still
  available, so a Rivet that can't run (no model, say) never strands it.

## Which admin agent

At request time the backend picks the admin agent whose Secrets extension
is on: Rivet when it qualifies, else the first such admin in the settings
order; none → `admin_agent_id` empty and the whole feature falls back to
today's flow. The pick is re-checked at approval (the agent may have been
deleted meanwhile); gone → the card falls back to the direct dialog.

## What the person sees

- **Mac**: the card reads like a request, not a form: "Sam needs an
  Anthropic API key — one that can run Claude models a few times. Why:
  …" with Approve / Dismiss, variables in small type, Choose a secret…
  underneath. After Approve, the card shows "Passed to Rivet." and later
  "Rivet gave it Anthropic key (sk-…a1b2)." A collect card appears in
  Rivet's conversation like the pairing card: prefilled fields, hidden
  value entry, Save and Cancel.
- **Phone**: the same card through Needs you; Approve and Dismiss are
  the primary actions, the fine print and the direct path behind the
  card's detail view. The collect card works there too — values typed on
  the phone go to the vault over the tunnel like any DecideSecretRequest.

## What stays untouched

- The requesting agent's tools: `secrets_request`, `secrets_requests`,
  `secrets_request_update`, `secrets_request_cancel` keep their shapes.
- The vault: crypto, grants, aliases, uses, the unlock window.
- No agent ever sees a value — the collect card's values go from the
  person's device into the vault, as today's add-new dialog's do.
- The asker's decision event, bash's `secrets`, and expiry semantics for
  PENDING requests.
- Connect proposals (`secrets_propose_connection`) are a separate flow
  and keep working as they are.

## Build order (after review)

1. Backend: states, fields, the admin pick, the approval event,
   `secrets_grant` with `request`, `secrets_collect` and its card state,
   fallbacks; tests in tests/all/vault.rs. (Sam)
2. `scripts/gen-proto.sh`, field list to the app side.
3. App: the approval card and the collect card, Mac and phone — after
   Zoe's wording fix to the current card lands, which this supersedes.
4. Rivet's prompt and the manual's ext-secrets and vault notes; Hana
   reviews the card text.
