# crew

A terminal dashboard for running, **chatting with**, and **managing** coding
agents across harnesses (Claude / OpenCode / Cursor). Rust + [ratatui].

Each agent runs its harness's interactive REPL inside a **PTY that the dashboard
owns**, rendered in an **embedded terminal pane** (via `portable-pty` + `vt100`). Select an agent and press `enter` to drop into its terminal — you
chat **inside the TUI**, side-by-side with the repo/agent tree; `Ctrl-q q` returns to the
dashboard. No switching windows. With tmux installed, agents keep running after
you quit crew and re-attach on the next launch.

<img width="2940" height="1840" alt="image" src="https://github.com/user-attachments/assets/8d1f4445-0cbe-4b78-add5-e9e9b10576b4" />


## Standalone — no backend required

crew runs on its own: it creates git worktrees **natively** (a `crew/<task>`
branch under `.crew/worktrees/`), keeps its own registry in `.crew/agents.json`,
and needs only `git` + whichever agent CLI you launch (`claude`, `opencode`,
`cursor-agent`). From the dashboard you can review diffs, tail logs, and
**commit + push + open a PR** (`gh`) — all without any external service.

**Agents outlive crew (tmux backend).** With `tmux` installed (default
`backend = auto`), every agent runs in a detached session on crew's private
tmux server (`tmux -L crew`, your `~/.tmux.conf` is not loaded; no status
bar, no prefix key). crew's chat pane is just a viewer: **quitting crew only
detaches** — agents keep working — and the next `crew` re-attaches them as
`● running`. You can also take one over from any terminal or over SSH:
`tmux -L crew attach -t crew-<agent-id>`. Without tmux (or `backend = pty`)
agents run inside crew and stop when it exits (it asks first).

Pick the backend in settings (`,` → *backend*: auto / tmux / pty), or per run
with `CREW_BACKEND=pty crew`. `crew --doctor` shows which one is active.

## Themes

Settings (`,` → *theme*, live preview, `esc` reverts) or `CREW_THEME=dracula crew`.
Built in: `default` (your terminal's colors), `catppuccin` (mocha), `dracula`,
`gruvbox`, `nord`, `tokyonight`, `onedark`, `solarized` — palettes from each
project's MIT-licensed terminal ports. A theme is a terminal palette (fg, bg,
ANSI 0–15) applied to the whole screen, embedded agent terminals included;
256-color and truecolor output passes through unchanged, as in any terminal.

**Resume.** Each launch is persisted to `.crew/agents.json` and its worktree
kept. An agent that exits or is stopped shows as **▷ resumable** (its last
screen stays in the preview) — `enter` relaunches it in the same worktree.
Claude agents are pinned to their own conversation (`--session-id` at launch,
`--resume <id>` later), so two agents sharing a folder never resume each
other's chat; opencode and cursor continue their latest session
(`--continue` / `--resume`). Overridable via `CREW_RESUME_CMD_<HARNESS>`.


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

`vendor/vt100` is vt100 0.15.2 with a one-line scrollback fix (see
`vendor/vt100/CREW_PATCH.md`); drop it when moving to ratatui 0.30.


```
cargo build --release
```

The binary lands at `target/release/crew`. Regenerate the website after UI
changes with `python3 site/gen.py` (captures live `--snapshot` frames).

**Prebuilt binaries.** Pushing a `v*` tag builds and attaches
`crew-<tag>-<target>.zip` (macOS arm64/x64, Linux x64) to a GitHub
Release via `.github/workflows/release.yml`.

### Install from a release

Download the zip for your platform, extract it, and put `crew` on your `PATH`:

```sh
# macOS (Apple Silicon)
curl -LO https://github.com/dropdevrahul/crew/releases/download/v0.0.1/crew-v0.0.1-macos-arm64.zip
unzip crew-v0.0.1-macos-arm64.zip
sudo mv crew /usr/local/bin/

# macOS (Intel)
curl -LO https://github.com/dropdevrahul/crew/releases/download/v0.0.1/crew-v0.0.1-macos-x64.zip

# Linux (x86_64)
curl -LO https://github.com/dropdevrahul/crew/releases/download/v0.0.1/crew-v0.0.1-linux-x64.zip
```

**macOS Gatekeeper:** if macOS says *"Apple could not verify 'crew' is free
of malware"*, run:

```sh
xattr -d com.apple.quarantine /usr/local/bin/crew
```

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

Two panes: a tree of pinned repos with each repo's agents nested under it (plus
an **all** row), and a live preview / embedded chat. Real output of
`CREW_SNAPSHOT_MODE=demo crew --snapshot`:

```
 crew  myapp  main · 5 agents  ●2 ✓2 ✗1  $1.050  ◉ 2 marked
╭ repos (1) · agents (5) ────────╮╭ preview ───────────────────────────────────────────────────────╮
│  ◆ all                         ││add billing dashboard endpoint                                  │
│  ▾ › myapp                     ││                                                                │
│▶   ⠹   add billing d… $0.42 12s││status   ⠹ running                                              │
│  ◉ ⠹   add auth midd… $0.31 40s││repo     myapp                                                  │
│  ◉ ✗   fix flaky time… $0.08 2m││harness  cursor · sonnet                                        │
│    ✓   rewrite the R… $0.05 10m││branch   crew/billing-api                                       │
│    ✓   add dark-mode … $0.19 1h││cost     $0.420                                                 │
│                                ││running  12s ago                                                │
│                                ││worktree .crew/worktrees/billing-api                            │
│                                ││                                                                │
│                                ││[enter] log · [d]iff [L]og [e]dit [!]shell [P]ush [R]etry [x]rm │
│                                ││                                                                │
╰────────────────────────────────╯╰────────────────────────────────────────────────────────────────╯
[?]help [n]ew [enter]chat/resume [q]uit [j/k]move [h]repo [d]iff [P]ush [s]top [x]rm [z]oom
```

`n` opens the new-agent form (`CREW_SNAPSHOT_MODE=new`):

```
╭ new agent ───────────────────────────────────────────────────────────────╮
│▶ task *                                                                  │
│┌────────────────────────────────────────────────────────────────────────┐│
││add auth middleware                                                     ││
│└────────────────────────────────────────────────────────────────────────┘│
│  repo      ‹ myapp ›  1/1 · launched here                                │
│  harness   ‹ claude ›  ⚠ untrusted: claude asks, pick Yes                │
│  options   sonnet · new git worktree (isolated)  [tab]                   │
│  images    (none — [a] add, [d] remove)                                  │
│  → new branch crew/add-auth-middleware-… off main                        │
│  [enter] launch [alt-enter] newline [tab] field [h/l] pick [esc] cancel  │
╰──────────────────────────────────────────────────────────────────────────╯
```

## Features

- **All repos, one list** — pin any number of repos (`w`). crew opens on
  **◆ all**: every workspace's agents merged and sorted by urgency —
  `⏸ waiting for input` first, then running, failed, resumable, done. The repo
  shows on each row. `h` → pick one repo to narrow the list; `a` jumps
  back to all. Actions (chat, diff, push, stop…) work on any row in either view;
  marks and bulk ops span repos.
- **Launch folder = default repo, not the view** — running `crew` inside an
  unpinned git repo pins it quietly (marked `›`) and pre-selects it in the
  new-agent form; the view still opens on all.
- **Waiting detection** — a live agent that printed output and then went quiet
  for 5s is sitting at its prompt: it's flagged `⏸`, sorted to the top, and you
  get a header toast (`⏸ … needs input`), even for agents in other repos.
- **New-agent form** — one required field. Pick **repo** and **harness**;
  model / worktree / workflow fold into one *options* line until you `Tab`
  into them. The form says where the work lands before anything is created
  (`→ new branch crew/<task>-… off main`). `Enter` launches, `Alt-Enter` adds
  a newline.
- **Worktrees that work** — each agent gets `crew/<task>` in
  `<repo>/.crew/worktrees/`. Top-level `.env*` files are copied in, and an
  optional `<repo>/.crew/setup` shell script runs in the new
  worktree (visible in the chat pane) before the harness starts — install deps
  there. `.crew/` is added to `.git/info/exclude`, so it never shows up in
  `git status`. Plain (non-git) folders run agents in place.
- **Embedded chat** — `enter` drops into the agent's terminal; `z` zooms the
  pane to full width; `Ctrl-q` leaves (Ctrl-o stays with Claude Code).
  - **Paste** is bracketed: multi-line text arrives as one message, never
    line-by-line. Pasting on the list opens a new agent with it as the task.
  - **History**: mouse wheel over the chat (or `Shift-PgUp/PgDn`) — forwarded
    to the agent when it handles the mouse itself (Claude's fullscreen UI),
    otherwise crew scrolls the terminal's 2000-line scrollback. Typing returns
    to the live view.
  - **Copy**: drag across the chat to select; releasing copies the text to the
    system clipboard (`pbcopy` / `xclip` / `wl-copy`). The wheel still scrolls history.
- **Notifications** — when an agent finishes or starts waiting, crew rings the
  terminal bell and posts a desktop notification (OSC 9: iTerm2, kitty,
  WezTerm, Ghostty), so you hear about it from another window.
- **Claude trust prompt** — a repo Claude Code already trusts covers its agent
  worktrees. For an untrusted repo, Claude's first screen asks, with **No**
  pre-selected: the form warns you; pick Yes (`↓` `Enter`), or run `claude`
  once in the repo root to trust it for every future agent.
- **Clean agent env** — if crew itself runs inside a Claude Code session, that
  session's markers (`CLAUDECODE`, `CLAUDE_CODE_SESSION_ID`, messaging
  socket/token, …) aren't passed to agents.
- **Resume** — stopped/exited agents turn `▷` immediately; `enter` resumes the
  same conversation (see *Quit & resume*). Observed Claude sessions can be taken
  over with `enter`.
- **Review & ship** — colored diff (`d`), live log tail (`L`), commit + push +
  PR (`P`), open PR/branch (`O`), editor/shell in the worktree (`e` / `!`).
- **Empty state** shows what crew found (git repo, which harness CLIs are on
  `PATH`) and the next key to press.

## Keys

| key | action |
|-----|--------|
| `j`/`k` or `↑`/`↓` | move through the repo → agents tree · `g`/`G` top/bottom · `Ctrl-d`/`Ctrl-u` half page · `Ctrl-f`/`Ctrl-b` page |
| `h`/`l` or `←`/`→` | `h` climbs from an agent to its repo · `l` steps from a repo into its agents (or opens the agent) |
| `]`/`[` | next/prev **failed** agent |
| `space` | mark / unmark the row and advance |
| `Ctrl-q` | **leader** key, on the dashboard and in a chat (see *Leader key* below) |
| `A` | mark all visible rows for bulk ops |
| `Ctrl-p` · `Ctrl-Tab` | agent picker (see *Agent picker* below); Ctrl-Tab also works inside a chat, in terminals that report it (kitty protocol: Ghostty, kitty, WezTerm, iTerm2) |
| `esc` | peel one layer: marks → search → category |
| mouse click / wheel | select a row · scroll · click the terminal pane to chat |
| `1` `2` `3` `4` | quick-filter: all / running / agents / sessions |
| `t` | cycle sort: status → cost → recent |
| `n` | new agent form — `Enter` launches, `Alt-Enter` newline, `Tab`/`j`/`k` between fields, `h`/`l` change pickers |
| `enter` / `l` | chat with a live agent · **resume** a `▷` one · take over an observed Claude session · open a headless agent's log |
| `Ctrl-q` | leave the chat, back to the dashboard |
| `z` | zoom the chat/preview pane to full width (toggle) |
| `a` | back to the **all repos** view |
| `shift-enter` | (in chat) newline in the agent's input (needs a terminal with the kitty keyboard protocol: Ghostty, kitty, WezTerm, iTerm2; `alt-enter` works everywhere) |
| `shift-tab` | (in chat) forwarded to the agent — cycles Claude Code's modes (plan / auto-accept) |
| `D` | duplicate the selected agent into a prefilled new-agent form |
| `w` | pin another folder as a workspace (`x` in the repos pane unpins; the picker opens beside the current repo) |
| `q` | quit (always confirms) — tmux agents keep running; pty agents stop and stay resumable |
| `d` | review the worktree diff; `/` searches, `n`/`N` cycle matches |
| `L` | tail a headless agent's log — live-follows at the bottom; `G` re-follows, `/` search, `r` reload |
| `P` | commit + push the worktree (or **all marked**) and open a PR (`gh`) |
| `R` | **retry / re-delegate** the task to a harness you pick (needs engine) |
| `s` / `S` | stop selected/marked · stop **all** (confirmed; they stay resumable) |
| `o` / `y` / `Y` | worktree path · yank path · yank branch |
| `v` | **review** what the agent changed (vs where it forked, committed or not) in nvim + diffview.nvim; see *Search & launchers* |
| `e` / `!` | open the worktree in your editor (default nvim) · drop to a `$SHELL` there |
| `O` / `f` | open the agent's PR (or branch) in the browser · `git fetch` |
| `,` | settings — persisted default harness/model/workflow/worktree |
| `r` | rename / label a crew-launched agent |
| `x` | remove the selected agent — or **all marked** (confirmed; no longer resumable) |
| `c` | forget finished (exited + resumable) rows (confirmed) |
| `/` | search · `H` hide/show observed sessions |

### Leader key

Press the leader, then one key. It is `Ctrl-q`, on the dashboard and inside a
chat. A which-key popup lists the bindings:

| after the leader | action |
|---|---|
| `q` | back: leave the chat, or quit (confirms) from the dashboard |
| `Tab` | agent picker |
| `n` · `d` · `P` | new agent · diff · push |
| `s` · `z` | stop · zoom |
| `?` · `,` | help · settings |
| `v` · `f` · `/` | review changes · find files · grep (see *Search & launchers*) |

Remap it in `~/.config/crew/workspaces.json` (or `$XDG_CONFIG_HOME/crew/`) with a
`leader` block. `key` is the dashboard leader, `chat_key` the in-chat one, and
`bindings` maps a key to an action name (`back`, `agents`, `new_agent`, `diff`,
`push`, `stop`, `zoom`, `help`, `settings`, `review`, `grep`, `files`). Key specs: `space`, `tab`, `enter`,
`esc`, `ctrl-<key>` (including `ctrl-space`; some terminals and macOS swallow it), or a single character. Missing fields
keep their defaults, but a `bindings` list replaces all of them.

```json
"leader": {
  "key": "ctrl-g",
  "bindings": [["q", "back"], ["tab", "agents"], ["n", "new_agent"], ["d", "diff"],
               ["P", "push"], ["s", "stop"], ["z", "zoom"], ["?", "help"], [",", "settings"]]
}
```

To use `Space` as the dashboard leader instead, set `"key": "space"` (it then no
longer marks rows). Inside a chat Space is typed text, so keep a `ctrl-` key for `chat_key`.

### Search & launchers

Search runs in the selected agent's worktree (on a repo line: the repo) and opens
hits in **your** editor at the matching line: `"editor"` in
`~/.config/crew/workspaces.json`, else `$VISUAL`/`$EDITOR`, else nvim. Terminal
editors take over the screen and return you to crew when you quit them; it works
from inside a chat too.

| key | does |
|---|---|
| `Ctrl-q f` | **find files**: fuzzy, gitignore-aware (`rg --files`), with a preview |
| `Ctrl-q /` | **live grep** (ripgrep, smart-case regex), preview centred on the hit |
| in search | type to filter · `Tab` files ↔ grep · `↑`/`↓` or `Ctrl-n`/`Ctrl-p` · `Enter` open in editor · `Ctrl-y` copy `path:line` · `Esc` close |
| `v` · `Ctrl-q v` | **review** the agent's changes vs its fork point: nvim + `:DiffviewOpen <base>` (falls back to fugitive's `:Git difftool`) |
| `e` | open the worktree in your editor |

Grep needs `rg` on your PATH; review wants
[diffview.nvim](https://github.com/sindrets/diffview.nvim) (vim-plug:
`Plug 'sindrets/diffview.nvim'`). Swap the review tool with `commands.review`, run
through `sh -c` in the worktree with `{base}` and `{worktree}` filled in:

```json
"editor": "hx",
"commands": { "review": "lazygit -p {worktree}" }
```

### Agent picker

`Ctrl-q Tab` opens the agent picker, as does `Ctrl-Tab` (also from inside a chat, where the terminal supports it) or `Ctrl-p` on the dashboard. Type to filter by repo, task, harness or status;
every word you type must match. `Up`/`Down` or `Ctrl-n`/`Ctrl-p` move, `Enter`
opens the agent (a live agent goes straight to its chat), and `Esc` cancels.

### New-agent form

- **task** — multiline editor. `Enter` launches, `Alt-Enter` (or `Ctrl-j`)
  inserts a newline. `Ctrl-S` also launches.
- **repo** — which pinned workspace to launch in (defaults to the one you're
  viewing, else the folder you started crew from).
- **harness** — claude / opencode / cursor, with an installed ✓/✗ check.
- **options** (folded until focused): **model**, **runs in** (new isolated git
  worktree / this repo checkout; plain folders always run in place), and
  **workflow** — only shown when the engine is present:
  `chat`, `plan → build (same harness)`, `plan → build on cursor|opencode`,
  `plan only (stop for review)`.
- **images** — attach image paths (`a` add, `d` remove); appended to the prompt.

Row glyphs: `⏸` waiting for input · `●` running · `✓` done · `✗` failed · `▷` resumable · `±` uncommitted
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
