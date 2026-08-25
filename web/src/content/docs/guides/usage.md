---
title: Usage & keys
description: How to launch zoetrope for Claude Code or Codex sessions, choose a provider, and use the full key map for scrubbing, camera, and overlays.
---

The launch only picks the **defaults**: *what* to open and *where the playhead
starts*. Once it's running, scrub / follow / pause / go-live are all available no
matter how you launched.

## Launching (native)

```text
zoe                          follow the current project's live session
zoe <dir>                    follow another project's live session
zoe <file.jsonl>             replay a recording from the start, paced
zoe <file.jsonl> --follow    open a recording at its live edge instead
zoe <file.jsonl> --speed N   playback speed multiplier (default 8.0)
zoe inspect <file.jsonl>     print the session tree and exit (no TUI)

zoe --provider claude        use Claude Code's local session store
zoe --provider codex         use Codex CLI's local session store
zoe --provider auto          detect a known provider layout or record shape
zoe --provider codex <dir>   follow a Codex session directory
zoe --provider auto <file>   detect the provider for one explicit file
zoe inspect --provider codex <file.jsonl>
```

A **file** target bulk-loads then tails it; a **directory** (or none → the current
project) discovers the latest session and follows it live. `--follow` only changes
where the playhead starts (the live edge instead of the beginning). An explicit file
is always read as that session and does not mix neighboring provider files. `auto`
uses known layout and record markers; if a path is ambiguous, choose `claude` or
`codex` explicitly.

### Session locations

Claude Code's default session store is `~/.claude/projects/<sanitized-cwd>/`.
Codex CLI's default session store is `$CODEX_HOME/sessions/YYYY/MM/DD/`, with
`$CODEX_HOME` defaulting to `~/.codex`. Set `CODEX_HOME` before launching to inspect
another Codex store, or pass the exact file/directory path directly. The app is
read-only and does not ask either provider for credentials.

## Launching (browser)

The [browser app](/app) boots into a bundled demo. To watch your own session:

- **Sessions** (Chromium browsers): click **Sessions**, choose Claude Code or Codex,
  pick a local folder, and follow it live when the browser supports the File System
  Access API. zoetrope reads the selected provider's transcript files and tails the
  folder for new activity. Nothing from the selected logs is uploaded by zoetrope.
- **Sessions** (other browsers): the same button falls back to a folder picker,
  so browsing and replaying work everywhere. **Following live does not** — without
  the File System Access API the browser hands over an immutable *snapshot* of
  each file, so writes that happen after you pick never arrive. The picker says
  so before you choose. Live-follow needs Chrome or Edge (or the native TUI).
- **Drag and drop** a `.jsonl` transcript (any browser). A drop carries only what
  you dropped. For Claude, drag the `<uuid>.jsonl` **and** its `<uuid>/` folder together
  to include sidecar subagents and workflows. For Codex, include the rollout files you
  want to inspect and choose the provider if auto-detection is ambiguous.

### Privacy when using the browser

You choose the folder or files in the browser picker; zoetrope does not discover or
read unrelated folders. JSONL can contain prompts, working directories, file paths,
tool inputs and outputs, source snippets, and model metadata, so treat it as sensitive
and do not drop raw logs into public issues. The selected log bytes stay in the page and
are not uploaded by the app. The hosted page may still request its own assets, fonts,
or analytics.

## Keys

| Key | Action |
| --- | --- |
| <kbd>space</kbd> | play / pause (resumes from the playhead) |
| <kbd>[</kbd> / <kbd>]</kbd> | jump to the previous / next prompt era |
| <kbd>End</kbd> / <kbd>g</kbd> | jump to the live edge |
| <kbd>s</kbd> | toggle skip-idle-gaps (compress dead air ↔ real-time) |
| mouse drag | seek along the scrubber |
| <kbd>o</kbd> / <kbd>f</kbd> | camera: Overview / Follow |
| <kbd>r</kbd> | relayout (tidy the graph) |
| arrows / <kbd>Tab</kbd> / <kbd>shift-Tab</kbd> | move between agents |
| <kbd>h</kbd> <kbd>j</kbd> <kbd>k</kbd> <kbd>l</kbd> | pan the graph |
| <kbd>+</kbd> / <kbd>-</kbd> / <kbd>0</kbd> | zoom in / out / reset |
| <kbd>c</kbd> | center on the selected agent |
| click | open an agent's detail panel |
| <kbd>j</kbd> / <kbd>k</kbd> / <kbd>PgUp</kbd> / <kbd>PgDn</kbd> | scroll the detail panel |
| <kbd>i</kbd> | session info overlay |
| <kbd>?</kbd> | help overlay |
| <kbd>esc</kbd> | close an overlay / clear the selection |
| <kbd>q</kbd> / <kbd>ctrl-c</kbd> | quit (native) |

<kbd>j</kbd> / <kbd>k</kbd> scroll the detail panel when an agent is selected, and
pan the graph otherwise.

### With a mouse

Almost everything above has a key, but the mouse is how most of it feels natural
— and **dragging the scrubber is mouse-only**: it is the one interaction with no
keyboard equivalent (the keys step era-to-era; the drag seeks continuously).
Wheel-zoom also differs from <kbd>+</kbd>/<kbd>-</kbd>: it anchors on the pointer
rather than the viewport centre.

Drag empty canvas to pan · wheel to zoom where you point · click an agent for its
provenance · drag the scrubber to travel through the session. There's a
[recording of all four on the front page](/#).

## Transport states

zoetrope never stores a "mode". The transport badge is *derived* from where the
playhead sits relative to the live edge:

- **Live:** following the edge, with appends arriving right now.
- **Playing:** paced replay moving forward through buffered events.
- **Paused:** paced replay, halted with <kbd>space</kbd>.
- **History:** parked in the past, scrubbed back off the edge.
- **Idle:** at the edge with no fresh activity, such as a finished or quiet session.

## Session info

Press <kbd>i</kbd> for the session overlay: mode, permission mode, queued
operations, file edits, and the last prompt. This data stays off the timeline and
shows only when you ask for it. The same data is available headless via
`zoe inspect <file.jsonl>`.
