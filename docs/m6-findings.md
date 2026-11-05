# M6 findings: Linear

Date: 2026-11-05. The installed `linear` CLI is 3.1.0 (checked with `linear --help` and `linear markdown`).

## API key

- The app uses a Linear personal API key, sent as `Authorization: <key>` to `https://api.linear.app/graphql`. Upstream uses OAuth through its cloud API, which a local-only app can't host.
- Settings checks the key with a `viewer` query before saving it.
- The key is stored in the macOS Keychain as a generic password: service `dev.teguh.humanlayerlocal`, account `linear-api-key`. It uses the `keyring` crate with `apple-native`.
- It is read once per app run and then cached in memory. The UI never gets the key back; it only learns whether one is saved.
- Errors from the HTTP client drop the URL (`without_url`), and the key never appears in an error, log or event.
- Dev builds are re-signed on every rebuild, so macOS may ask once per rebuild whether the app can read the Keychain item.

## The `linear` CLI in sessions

- None of the bundled RPI skills call `linear`. They only mention Linear tickets as context.
- The installed `linear` 3.1.0 reads `LINEAR_API_KEY`. That is a different CLI from upstream `apps/linear-cli`, but it uses the same variable.
- Every session process gets `LINEAR_API_KEY` from the Keychain on top of the shell environment. An agent that runs `linear issue view ENG-1` then uses the same key.
- The agent can see the key through its environment. That is the trade-off the plan asked for ("same API key").

## Import

- Accepted: an ID such as `ENG-123`, or an issue URL such as `https://linear.app/<org>/issue/ENG-123/<slug>`.
- The task is created as follows:
  - Its title and description come from the ticket.
  - Its slug is `eng-123-<title>`.
  - It goes at the bottom of the project's backlog.
  - Each ticket can be imported once per project (a unique index).
- `ticket.md` in the docs folder holds the status, assignee, branch, parent, link, description and comments (up to 50), in the spirit of upstream `format-linear-ticket.ts`.
- Images in the description and comments are downloaded to `images/img-eng-123-<n>.<ext>` in the docs folder, and `ticket.md` links the local copies.
  - Linear uploads (`uploads.linear.app`) are fetched with the key; other hosts get no Authorization header.
  - An image that fails to download keeps its original link, and the import says how many failed.
- The research prompt for a linked task points the agent at `ticket.md` and `images/`.
- The docs viewer shows local images through a `data:` URL command. It never loads remote images.

## Sync (every 60 seconds)

- While the app is open, one batched GraphQL request (aliases, 25 issues each) fetches every linked task that isn't archived.
- When a ticket's `updatedAt` changes, the app updates the state name and rewrites `ticket.md` and its images.
- Title and description follow the ticket only while they match what was last imported. A local edit keeps that field from being overwritten.
- Errors and missing tickets show in Settings, along with the time of the last check. "Check now" runs a check immediately.

## Stage → status

- Ported idea from upstream `linear-status-progression.ts`: moves go forward only. States are ranked by category (triage < backlog < unstarted < started < completed < canceled), then by board position. Canceled is terminal.
- Each stage tries upstream's RPI column first (from its `ensure-columns`), then Linear's default name, then the first state of a category:

| Stage | Tried in order |
|---|---|
| research | `research-in-progress`, then the first started state |
| plan | `plan-in-progress`, then the first started state |
| implement | `in-dev`, `In Progress`, then the first started state |
| review | `code-review`, `In Review`, then the first started state |
| done | `Done`, then the first completed state |
| backlog | none |

- The move runs in the background after the board move. Success shows "Moved ENG-1 to …" in the notice bar; failure shows in the error bar.

## Docs on the ticket (deviation)

- The docs live only on this machine, so there is no URL to link to.
- Instead, each doc has a **Post to Linear** button. It posts the doc as a comment: a bold file name, then the doc in a collapsible `+++ [name]` section (Linear-flavored Markdown, per `linear markdown`). Posting again updates the same comment, and a deleted comment is posted fresh.
- It is a button, not automatic, because posting sends doc content to the team's Linear.

## Tests

- `hl-linear`:
  - The client is tested against a local mock HTTP server: batch query, not found, auth error, the header, and identifier upper-casing.
  - The status mapping is tested for the default and RPI workflows.
  - Ticket parsing, image handling and `ticket.md` are tested.
- `hl-core`: import, duplicate refusal, sync that respects local edits, and posted-comment records.
- Not tested automatically: real Linear calls. They need the user's key, which is entered only in the app.
