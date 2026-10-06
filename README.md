# crew

A terminal dashboard for running, **chatting with**, and **managing** coding
agents across harnesses (Claude / OpenCode / Cursor). Rust + [ratatui].

Each agent runs its harness's interactive REPL inside a **PTY that the dashboard
owns**, rendered in an **embedded terminal pane** (via `portable-pty` + `vt100` +
[tui-term]). Select an agent and press `enter` to drop into its terminal — you
chat **inside the TUI**, side-by-side with the agent list; `Ctrl-o` returns to the
dashboard. No tmux, no switching windows.

## Standalone — no backend required

crew runs on its own: it creates git worktrees **natively** (a `crew/<task>`
branch under `.crew/worktrees/`), keeps its own registry in `.crew/agents.json`,
and needs only `git` + whichever agent CLI you launch (`claude`, `opencode`,
`cursor-agent`). From the dashboard you can review diffs, tail logs, and
**commit + push + open a PR** (`gh`) — all without any external service.

**Quit & resume.** Live agents end when you quit (confirmation first). Each launch
is persisted to `.crew/agents.json` and its worktree kept, so on the next run the
agent shows as **▷ resumable** — `enter` relaunches the harness with its resume
flag (`claude --continue`, `opencode --continue`, `cursor-agent --resume`) in the
same worktree. Overridable via `CREW_RESUME_CMD_<HARNESS>`.

[tui-term]: https://crates.io/crates/tui-term

## Compared to other TUIs

Inspired by lazygit / k9s / claude-squad; built for agents.

| capability | crew | claude-squad | lazygit | k9s |
|---|:--:|:--:|:--:|:--:|
| multiple agents, one view | ✓ | ✓ | ✓ | · |
| embedded chat (no tmux) | ✓ | — | — | — |
| per-agent git worktree | ✓ | ✓ | — | — |
| cross-harness | ✓ | · | — | — |
| diff viewer | ✓ | ✓ | ✓ | · |
| live log follow | ✓ | · | — | ✓ |
| commit · push · PR | ✓ | · | ✓ | — |
| multi-select bulk ops | ✓ | · | — | ✓ |
| runs standalone | ✓ | ✓ | ✓ | ✓ |

## Build

```
cargo build --release
```

The binary lands at `target/release/crew`. Regenerate the website after UI
changes with `python3 site/gen.py` (captures live `--snapshot` frames).

**Prebuilt binaries.** Pushing a `v*` tag builds and attaches
`crew-<tag>-<target>.tar.gz` (macOS arm64/x86_64, Linux x86_64) to a GitHub
Release via `.github/workflows/release.yml`.

## Run

From anywhere inside a repo (it resolves the git toplevel and scopes to it):

```
crew                              # interactive dashboard
crew --new "fix the bug"          # create a worktree + run the harness headless in it
crew --new "…" --harness cursor --model auto
crew --new "quick fix" --here     # run in the current repo (no new worktree)
crew --dump                       # print the agent list once and exit (no TTY)
crew --snapshot                   # render one real frame to text (visual smoke, no TTY)
crew --doctor                     # check git / harness CLIs / gh
```

`--new` is fully standalone: it makes a `crew/<slug>` worktree and runs the
harness one-shot in it (streaming to your terminal), then prints the branch +
worktree path.

Environment:
- `CREW_SCRIPTS` — optional engine scripts dir (same as `--scripts`).
- `CREW_PYTHON` — python interpreter for the optional engine (default `python3`).
- `CREW_HEADLESS_CMD_<HARNESS>` — override the `--new` headless command.
- `CREW_INTERACTIVE_CMD_<HARNESS>` / `CREW_RESUME_CMD_<HARNESS>` — override REPL / resume argv.

## Layout

A master-detail **explorer**: a selectable agent list on the left, a live
preview on the right.

```
┌ agents (2) ──────────┐┌ preview ─────────────────────────┐
│▶ ✓ cursor  add bill… ││ add billing dashboard endpoint   │
│  ● claude  fix auth… ││ status   ● running               │
│                      ││ harness  claude · sonnet         │
│                      ││ steps    1/2   cost $0.010       │
│                      ││ branch   crew/billing            │
│                      │└──────────────────────────────────┘
│                      │┌ log (tail · [enter] full) ───────┐
│                      ││ {"type":"result","tokens":10,…}  │
└──────────────────────┘└──────────────────────────────────┘
```

## Features

- **Explorer view** — managed agents + observed sessions in a scrollable list,
  **priority-sorted** (live work first, then failed, then done, sessions last),
  colored status icons (`●` running, `✓` done, `✗` failed, `■` stopped), each row
  showing step `k/n` and relative time. Selecting one shows a full preview + live
  log tail on the right.
- **Diff/verdict viewer** (`d`) — review exactly what an offloaded agent changed:
  the recorded unified diff (colored) plus the guardrail verdict.
- **Live step progress** — plan-linked agents show `k/n` done steps from
  `step_ledger`.
- **Live PID liveness** — `a` filters to agents whose process is actually alive.
- **Header totals** — agent/live counts and summed metered cost.
- **Incremental search** (`/`) over task / harness / id / status.
- **Launcher** — `n` opens a form where the **task is required**; harness is a
  picker and the model is prefilled with that harness's default. Launches in a
  fresh git worktree.
- **Log tail** auto-follows; `enter` opens a full-screen scrollable log.
- **Stop** asks to confirm; **`o`** reveals the worktree path.
- Auto-refreshes every ~1.5s.

## Keys

| key | action |
|-----|--------|
| `j`/`k` or `↑`/`↓` | move selection · `g`/`G` top/bottom · `]`/`[` next/prev **failed** |
| `space` / `A` | mark row / mark all for bulk ops · `esc` clears marks |
| mouse click / wheel | select a row · scroll the list |
| `1` `2` `3` `4` | quick-filter: all / running / agents / sessions |
| `t` | cycle sort: status → cost → recent |
| `n` | new agent form (multiline task, workflow/harness/model/worktree pickers, image attach); `Ctrl-S` to launch |
| `enter` | chat with a live agent, or **resume** a `▷` persisted one |
| `shift-tab` | (in chat) forwarded to the agent — cycles Claude Code's modes (plan / auto-accept) |
| `D` | duplicate the selected agent into a prefilled new-agent form |
| `Ctrl-o` | leave the terminal, back to the dashboard |
| `q` | quit (confirms if agents are running) |
| `d` | review the worktree diff (colored, scrollbar); `/` searches, `n`/`N` cycle matches |
| `l` | tail a headless/delegated agent's log — **live-follows** (tail -f) when parked at the bottom; scroll up to pause, `G` to re-follow, `/` search, `r` reload |
| `P` | commit + push the worktree (or **all marked**) and open a PR (`gh`) |
| `R` | **retry / re-delegate** the selected task to a harness you pick (runs headless in a fresh worktree) |
| `s` | stop the selected agent — or **all marked** if any (`y` to confirm) |
| `S` | stop **all** running agents (`y` to confirm) |
| `o` / `y` / `Y` | worktree path · yank path · yank branch |
| `e` / `!` | open the worktree in `$EDITOR` · drop to a `$SHELL` there |
| `O` | open the agent's **PR** (if one is open) or its branch on the remote |
| `,` | settings — edit persisted default harness/model/workflow/worktree |
| `L` | label / rename a crew-launched agent (shown instead of the task) |
| `x` | remove the selected agent — or **all marked** (confirmed) |
| `c` | clear finished (exited + resumable) rows |
| `/` | search · `h` toggle observed sessions |

### New-agent form

- **task** — a multiline editor (Enter inserts a newline).
- **workflow** — how the task runs, cycled with `←`/`→`:
  - `chat (single agent)` — plain interactive session.
  - `plan → build (same harness)` — seeds `/nx-plan` then `/nx-build`.
  - `plan → build on cursor` / `… on opencode` — seeds `/nx-build --harness X`,
    so the plan is decomposed then every step executed on that harness.
  - `plan only (stop for review)` — seeds `/nx-plan` and stops, so you review
    the plan before any code is written.
- **harness / model / worktree** — `‹ … ›` pickers, change with `←`/`→`.
  Worktree = *new isolated git worktree* or *current repo*.
- **images** — attach image paths (`a` add, `d` remove); appended to the prompt.
- **`Ctrl-S`** launches (Enter is reserved for the task editor).

Row glyphs: `●` running · `✓` done · `✗` failed · `▷` resumable · `±` uncommitted
changes · `◉` marked.

### Look & feel

Rounded panels with a focus-lit accent border, an animated braille spinner on
running agents, per-harness colored names (claude/cursor/opencode), inline cost,
a header status legend (`●running ✓done ✗failed`) + total metered cost + current
git branch, a btop-style context-fill meter in the detail pane, and scrollbars
on the diff/log viewers. `?` opens a grouped keybinding cheatsheet.

## Future work

- Auto route→harness offload (pick the cheapest capable harness per plan step).
- Interactive attach/takeover (tmux) for hands-on driving of a running agent.

[ratatui]: https://ratatui.rs
