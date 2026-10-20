# Smart routing

Read this before changing `src/routing/smart.rs` or `src/routing/look.rs`. The ladder that runs when the experiment is off is described in `docs/iphone-and-routing.md`, and the words used here are defined in `CONTEXT.md`.

Smart routing is an experiment, on with `"experiments": {"smartRouting": true}` in `~/.config/scrawl/config.json` (read through `handoff::experiment`). With it off the app calls `routing::route_capture` exactly as before. With it on, a Mac Screen or Window capture is routed in three steps.

1. `look::look_live` runs while the bundle is built. It reads the browser tab in front (or captured) and turns it into clues: the page's `/sdk.js` registration, and the folder of the process serving its port when the tab is on this Mac. "On this Mac" means a loopback host or one of this Mac's own names (`hostname`, its `.local` name, its Tailscale name), whose port is mapped through `tailscale serve status --json`. It reads Ghostty's focused pane (folder and title) when Ghostty is captured or in front. Then it reads the open root sessions: the newest `SESSION_PAGES` pages, the newest page in each folder a clue names, and each session a clue names.
2. Once the bundle exists, `look::bundle_clues` adds clues from it: the window title minus the app's own name, the captured app's name unless it's a browser, terminal, editor or system app, the checkout the captured app is built from, the transcript, and for OpenCode Desktop the session viewed last. The recorded destination becomes the address. `smart::decide(&evidence, Policy::Weigh)` returns a judgement. `routing::judge_capture` saves the evidence and decision as `routing.json` in the bundle and logs one line.
3. `ScrawlCoordinator::act_on_route` acts on it when it arrives, after the capture has been copied and shown as Completed (routing never delays that; ✕ or Paste before it arrives keeps the capture on the clipboard). Session reads are bounded as described in `docs/iphone-and-routing.md`.
   - A send shows "Sending to DESKTOP…" for `SMART_SEND_UNDO` (3 s), with the runner-up as a "Send to OPENCODE IOS" chip beside Paste. ✕ or Paste cancels the send; the chip sends there instead, at once. Then the destination is written to `scrawl.json` and `summary.md` and sent through `send_completed` (and so `delivery::deliver`, with its retries).
   - An offer asks: the Completed island keeps "Copied" and adds "Send to DESKTOP" and the runner-up ("OPENCODE IOS") beside Paste, which stays the default. Nothing is sent until a chip is clicked; a click is the user's choice (`via: "chosen"`). The review window's status names the suggestions too.

## The captured app's checkout

When the captured app is software whose source Kit works on, that checkout is a clue (`Source::App` with a folder, 70 points, like a terminal's folder). `look::app_checkout` finds it:

- `"routing": {"apps": {"<bundle ID or prefix>": "<folder>"}}` in config.json, for apps whose checkout is named otherwise (`"com.kitlangton.problems": "~/code/worktrees/team-board/rewrite"`). The longest matching prefix wins.
- Otherwise the one open session repository (or folder, outside a repository) whose name is the app's name run together or one of its words or bundle ID parts, generic ones dropped: OpenCode Beta (`ai.opencode.desktop.beta`) → `…/opencode`, Scrawl Helper (`dev.kitlangton.scrawl.helper`) → `…/scrawl`, Hex → `…/hex`. Two checkouts named alike give nothing. Browsers, terminals, editors and system apps give nothing, OpenCode aside.

There are no built-in paths: the defaults for OpenCode and Scrawl come from their checkouts being named `opencode` and `scrawl`, so they work on any Mac where a session is open in them.

## OpenCode Desktop: the session or the app?

Most captures of OpenCode Desktop are about OpenCode itself (the composer, a picker, the sidebar), not about the session it shows. `look::about` reads what was said, the Accessibility roles of what was clicked and what marks were drawn on:

- **Interface**: two or more interface words ("button", "composer", "picker", "spacing", "search"…) or clicks on controls, and no talk about the session's work. The app's checkout gets 70 and the viewed session 15.
- **Session**: phrases about the agent's work ("you said", "your plan", "this diff"…) at least as many as interface words. The viewed session gets 60 and the checkout 30.
- **Unclear**: anything else. The checkout gets 60 and the viewed session 30, so the viewed session never beats the app's own repo when it can't be told, and neither sends on its own.

## Scoring

A candidate's score from clues is the sum, over sources, of the best points any one clue of that source gives it. Recency is added to that if it's above zero. The points are in `Source::points` (or a clue's own `points`, as above) and the thresholds are `SEND_AT` (95), `LEAD` (15), `OFFER_AT` (40) and `ALT_AT` (15).

- Page (`data-session`: 100, `data-directory`: 95) and Port (95) send on their own. Terminal (70), the app's checkout (70), Document (60, not gathered yet), Viewed (60), App name (50), Title (40) and Speech (25) don't.
- A folder clue gives full points to the sessions closest to its folder (on the line from the project root to it, or inside it), 15 fewer per extra folder for sessions further along that line, and half to a session elsewhere in the project or in another checkout of the same repository (`look::repo_of` follows a worktree's `commondir`).
- A word clue's points are divided by the number of candidates the word names. A word names a session through its whole title or its folder's or repository's whole name (always), a word of a longer title, or a piece of its folder's name. Ordinary words (`smart::COMMON`: "loading", "search", "button", "chat"…) only count as a whole name, because folder and title words say what the work is about and the same words are said about everything. A piece of a worktree's name counts half, and OpenCode's made-up worktree names (`…/worktree/<hash>/lucky-rocket`) name nothing.
- Recency: 20 within 10 minutes of the capture's end, 12 within an hour, 5 within a day, counting the session's update or view times up to a minute after the end.

## Deciding

Candidates rank by their clue score; recency only breaks ties.

- **Send** when the best has at least `SEND_AT` from clues alone and leads every other candidate by `LEAD`, recency included. So recency can separate two sessions a port names equally, but never lifts a session to `SEND_AT`.
- **Offer** (ask) when the best scores at least `OFFER_AT` otherwise.
- **Keep** below that, with the reason.

A send and an offer both carry runners-up (`smart::alternatives`, at most two): a candidate within `LEAD` of the best, or one resting on a different source than the best with at least `ALT_AT` from clues, one per source. That's the session a misread clue would have meant: the viewed session when the app's checkout leads, or the reverse.

## Laws

The tests in `smart.rs` and `look.rs` check each one. An address always wins, and a choice beats a recorded destination. A send needs `SEND_AT` from clues and a lead of `LEAD`. `decide` is pure, so the same evidence gives the same decision, in any order and with clues repeated. Recency never puts a session forward on its own and never reaches `SEND_AT`. Adding support for the leader never demotes it. The home folder and `/` are never projects. One medium clue (the viewed session, a terminal folder, the app's checkout or name) can't send without a second that agrees. `look::tests::the_ladder_policy_reproduces_route` checks that `Policy::Ladder` over the same hooks decides what `routing::route` does on a table of cases.

Real captures replay as fixtures in `src/routing/fixtures/` (sessions trimmed): the OpenCode capture that was sent to OPENCODE IOS while Kit was talking about OpenCode's composer now asks about DESKTOP with OPENCODE IOS one click away; the same capture talking about the session suggests OPENCODE IOS; two dev sites and two Device Hub captures decide as they did.

## The pill

The Metal capsule (`src/native/pill`) draws the chips as `ElemId::Route0`/`Route1` from `Pres::Completed { label, routes }`, with labels rasterized into reserved atlas rows (`GlyphAtlas::update_route`) and reported as `PillAction::Route(i)`. A chip inserted between items that are still travelling waits until they've cleared its spot before it fades in. `native::pill::tests::route_chips_render_offscreen` draws every state offscreen; with `SCRAWL_PILL_STILLS=DIR` it writes them, and frames of the morph, as PNGs. The GPUI capsule shows the same chips. The SwiftUI pill (`feat/swiftui-pill`, `helper/Pill.swift`) would need `route:0`/`route:1` items with their labels in the state it's sent, drawn like `inspect` (the first with a faint fill when it reads "Send to …"), and to report a `route:<i>` action.

## Tools

- `scrawl route --explain <bundle|latest> [--url URL]` prints the evidence, the sessions weighed with their points, the decision with why it wasn't sent and what's one click away, and what the ladder does. It replays `routing.json` when the bundle has one, keeping the sessions and what was seen live (the tab, the port, the terminal) and reading the bundle's own clues again with today's rules. Otherwise it gathers what it can now (sessions as they are today, so recency and the viewed session are only approximate), and `--url` stands in for the tab.
- Tests swap the outside steps through `look::Senses` (which holds `routing::Hooks`), and never post to a session.
