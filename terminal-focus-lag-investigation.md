# Herdr focus lag and terminal color-response leakage

Investigation date: 2026-09-10 (America/Denver). Status: diagnostic findings; no code fix implemented or verified.

## Main finding

On this Mac, switching back to iTerm while using Herdr causes a several-second pause in scrolling and typing. A fresh iTerm tab outside Herdr does not exhibit the pause. After one pause, a terminal palette-color response appeared as literal text in the Claude Code input field.

The strongest working explanation is a problem in Herdr's focus-triggered appearance/palette query exchange with iTerm. Source inspection shows that gaining focus can cause an appearance query, whose response causes a burst of 258 color queries. Captures show blocked output in Herdr and a synchronization wait in iTerm's terminal reader. The visible response leakage confirms that terminal protocol data reached application input, but the exact parser failure and the causal connection between the query burst and every second of the pause have not yet been established.

An earlier hypothesis that the optional full redraw on focus was sufficient to explain the issue failed its configuration test. That change was reverted.

## Environment and scope

| Component | Observed value |
| --- | --- |
| macOS | 26.6.2, build 25G83 |
| Architecture | ARM64 |
| Logical CPUs / installed memory | 12 / 48 GiB |
| iTerm application | 3.7.0, from process sample |
| Herdr client and server | 0.9.0, stable |
| Herdr private protocol | 22; client/server compatible |
| Endpoint generation | 1 |
| Herdr binary state | Not stale; status reported no restart needed |
| Shell | zsh with Oh My Zsh, robbyrussell theme |
| Affected application inside Herdr | Claude Code |
| Comparison machine | User reports no issue with Herdr for Linux inside WSL2 on Windows |

The WSL2 machine was not inspected. Its Herdr version, outer terminal application, and settings were not verified; the platform comparison below describes the inspected v0.9.0 source.

All Herdr code findings were checked against the published `v0.9.0` checkout at commit `b99002ac99b09e00b4ca692436cb15a6b0d676f1`. The destination project was at `90e947a6e26e02f10ec7f1694c4eb5b78a86406b` when this report was written. Do not assume those revisions have identical behavior.

## User-observed reproduction

1. Use Herdr inside iTerm, with Claude Code in a pane.
2. Switch to another application.
3. Scrolling the unfocused terminal can still be responsive.
4. Give iTerm focus again.
5. Scrolling and keyboard interaction pause for several seconds.
6. When the pause ends, terminal control-response text can appear in Claude's prompt.

The user confirmed that the pause occurred during the 20-second focus capture. They also confirmed that a plain iTerm tab outside Herdr has no corresponding focus delay.

The screenshot supplied at approximately 10:02 shows this visible fragment:

```text
]4;101;rgb:8787/8787/5f5f
```

This matches the printable body of an OSC 4 palette-color response for index 101. A complete response normally includes an ESC introducer and a terminator; the screenshot does not establish which bytes were dropped, stripped, consumed as keys, or simply not displayed. It is not a raw byte capture. The important observation is that color-response text reached Claude's editable input rather than being consumed as host-terminal metadata.

## System and shell checks

- A short system sample showed approximately 87% CPU idle. iTerm and Herdr had modest CPU use, not sustained core saturation.
- VM counters showed zero swap-ins and swap-outs. Memory compression was present, but there was no evidence of swap thrashing during the inspection.
- Shell startup measurements were approximately 0.46 seconds for a login interactive shell, 0.53 seconds for an interactive shell, and below the timer's 0.01-second resolution for a clean noninteractive shell.
- An additional Oh My Zsh profiling run re-sourced the configuration and spent much of its measured time in completion initialization and plugin sourcing. Because it re-sourced an already initialized shell, its breakdown is not a clean first-start profile.
- These startup costs do not explain a pause triggered by application focus after the shell is already running.
- iTerm's capture showed the Metal rendering path was active. Its profile had 50,000 scrollback lines, unlimited scrollback disabled, blur disabled, ASCII ligatures enabled, and approximately 2% transparency. No causal connection to those settings was demonstrated.
- A surviving `iTermServer-3.6.11` helper was present beneath the older shell session while the GUI was 3.7.0. This is an observation, not evidence that a version mismatch caused the problem; a fresh-tab comparison also changes session history.
- Herdr logs did not provide a current diagnostic identifying this focus pause. Older update-fetch and manifest warnings were not connected to the reproduction.

## Captured blocking behavior

The focus capture started at approximately 09:56:30 local time. Three processes were sampled concurrently for 20 seconds at a requested 5 ms interval:

| Process | PID during capture | Relevant observation |
| --- | --- | --- |
| iTerm | 73986 | Main thread mostly in its ordinary event wait; terminal reader accumulated 360 samples waiting on a semaphore in `TokenExecutor.addTokens` |
| Herdr client | 74100 | Main client loop accumulated 206 samples in the kernel `write` call while `ClientState::present_frame` wrote stdout |
| Herdr server | 73375 | Mostly waiting, with some rendering work; no comparable sustained CPU bottleneck identified |

Relevant iTerm reader stack:

```text
TaskNotifier.run
  PTYTask.processRead
    PTYTask.readTask:length:
      VT100ScreenMutableState.threadedReadTask:length:
        TokenExecutor.addTokens
          OS_dispatch_semaphore.wait(wallTimeout:)
            semaphore_wait_trap
```

Relevant Herdr client stack:

```text
run_client_loop
  ClientState::present_frame
    Stdout::write_all
      StdoutLock::write_all
        ...
          write
```

Multiplying counts by the requested interval gives about 1.8 seconds of sampled reader waiting and 1.03 seconds of sampled client writing. These are approximate accumulated occupancies, not measured continuous stalls. Sampling overhead and missed intervals limit precision. The captures aggregate stacks rather than providing a synchronized event timeline; the times must not be added together as the duration of a single freeze.

The Herdr client runs a current-thread async runtime and performs synchronous frame writes. A blocked frame write can therefore delay processing other events on that loop, including input. iTerm's reader wait is consistent with output backpressure, but its exact reason was not determined. A semaphore wait alone does not prove a deadlock.

## Source trace: focus to palette replies

The inspected release contains this sequence:

1. `RawInputEvent::OuterFocusGained` sets `outcome.query_host_appearance = true` in the client shell input handler. This is independent of the optional focus redraw setting.
2. The client shell runtime sends the appearance query `ESC [ ? 996 n`.
3. A parsed `HostColorSchemeChanged` response sets `outcome.query_host_theme = true`. The inspected branch does not first compare the reported appearance against the previously known appearance. Theme auto-switching controls palette/repaint behavior separately; it does not guard this query assignment.
4. `host_terminal_theme_query_sequence` sends foreground and background queries (OSC 10 and 11), plus one OSC 4 query for each palette index 0 through 255 when platform policy allows palette queries.
5. On macOS this policy returns `true`, yielding 258 individual color queries. Their replies share the terminal input stream with keystrokes.

Source references, pinned to the inspected release:

- [Focus and appearance event handling](https://github.com/herdrdev/herdr/blob/v0.9.0/src/client/shell/input.rs)
- [Query dispatch in client shell runtime](https://github.com/herdrdev/herdr/blob/v0.9.0/src/client/shell_runtime.rs)
- [Terminal query writes](https://github.com/herdrdev/herdr/blob/v0.9.0/src/client/terminal_geometry.rs)
- [Query sequence construction and response parsers](https://github.com/herdrdev/herdr/blob/v0.9.0/src/terminal_theme.rs)
- [Synchronous frame output](https://github.com/herdrdev/herdr/blob/v0.9.0/src/client/state.rs)

Herdr already contains mechanisms intended to prevent this kind of leakage. `RawInputByteFramer` tracks expected host replies, arms a bounded hold for split ESC introducers, recognizes appearance reports, and rearms color-reply tracking on scheme reports. Its comments explicitly mention an earlier split OSC reply leakage issue (#549). The Unix input reader also batches palette replies. These mechanisms exist; this investigation did not establish the particular byte split, timeout, interleaving, or state transition that bypassed them here. The reference to #549 is from a source comment, not a verified duplicate issue finding.

- [Raw input framing and tests](https://github.com/herdrdev/herdr/blob/v0.9.0/src/raw_input.rs)
- [Unix stdin reader and palette handling](https://github.com/herdrdev/herdr/blob/v0.9.0/src/client/input.rs)

## Why WSL2 can behave differently

| Runtime platform | Palette-query policy in v0.9.0 |
| --- | --- |
| macOS | Enabled |
| Linux outside WSL | Enabled |
| Linux inside WSL | Disabled after WSL detection |
| Native Windows | Disabled; separate client query policy also skips host theme queries |

Linux implements `should_query_host_terminal_palette()` as `!running_inside_wsl()`. Detection checks kernel identification, WSL environment markers, and `/run/WSL`. WSL's Unix path can still query foreground/background colors; it avoids the additional 256 palette queries.

This gives a concrete explanation for why the user's WSL2 setup avoids this particular large response burst. It does not prove the bug is exclusive to macOS: ordinary Linux enables palette queries too, and the outer terminal's behavior also matters.

- [macOS policy](https://github.com/herdrdev/herdr/blob/v0.9.0/src/platform/macos.rs)
- [Linux and WSL detection/policy](https://github.com/herdrdev/herdr/blob/v0.9.0/src/platform/linux.rs)
- [Native Windows policy](https://github.com/herdrdev/herdr/blob/v0.9.0/src/platform/windows.rs)

## Configuration experiment and restoration

The initial focus hypothesis concerned `ui.redraw_on_focus_gained`, which defaults to `true` and forces a full host-terminal redraw.

Experiment:

```toml
[ui]
redraw_on_focus_gained = false
```

The original configuration was backed up, this one setting was added, and `herdr server reload-config` returned `status: applied` with no diagnostics. The user reported that the pause persisted and supplied the color-response screenshot.

The added setting was then removed and the configuration successfully reloaded again. A byte-for-byte comparison against the original backup passed when writing this report. No configuration change from this experiment remains.

A follow-up sample attempt after the setting change did not capture the old Herdr client PID because that process was no longer present. The original PID must not be reused for future captures. The later client PID observed was 88148. The user-reported failed experiment, rather than that incomplete follow-up capture, is the evidence that disabling redraw did not resolve the issue.

No Herdr binary was rebuilt or replaced. No source implementation was modified, and no issue or PR was submitted. No successful mitigation has been verified.

## Evidence files

These are absolute local paths on the reporting Mac. They are not bundled into this project or publicly uploaded.

Primary focus captures and configuration backup:

```text
/Users/paul/Documents/Codex/2026-09-10/se/outputs/terminal-diagnostics/iterm-focus-sample.txt
/Users/paul/Documents/Codex/2026-09-10/se/outputs/terminal-diagnostics/herdr-client-focus-sample.txt
/Users/paul/Documents/Codex/2026-09-10/se/outputs/terminal-diagnostics/herdr-focus-sample.txt
/Users/paul/Documents/Codex/2026-09-10/se/outputs/terminal-diagnostics/herdr-config-before-focus-test.toml
```

Screenshot:

```text
/Users/paul/Desktop/Screenshot 2026-09-10 at 10.02.45 AM.png
```

Initial short samples and incomplete follow-up:

```text
/Users/paul/Documents/Codex/2026-09-10/se/work/iterm-sample.txt
/Users/paul/Documents/Codex/2026-09-10/se/work/herdr-sample.txt
/Users/paul/Documents/Codex/2026-09-10/se/work/herdr-client-sample.txt
/Users/paul/Documents/Codex/2026-09-10/se/work/iterm-focus-after.txt
```

## Remaining investigation and candidate fixes

These are proposed next steps, not tested fixes:

1. Capture timestamped query/reply metadata and framing decisions around a focus event, taking care not to record unrelated prompt contents or keystrokes. Establish whether repeated appearance responses produce redundant palette queries and exactly how a reply reaches pane input.
2. In an isolated test build, suppress palette re-querying when a focus-triggered appearance response is unchanged. Compare against disabling the 256 palette queries while retaining foreground/background queries. Measure focus latency and check for leaked input in both cases.
3. Exercise reply framing with fragmented OSC 4 responses, a separately delivered ESC, delayed terminators, coalesced replies, and interleaved input. Use the captured byte sequence when available rather than assuming the screenshot contains a complete response.
4. Investigate whether synchronous frame writes and input-channel backpressure can prevent the client from draining terminal replies promptly. Separate normal output waiting from any circular dependency with iTerm's reader.
5. Validate a fix in iTerm, a second macOS terminal, Linux, and WSL2. Include repeated focus changes with an unchanged theme, an actual light/dark theme change, and active pane output. Verify both latency and absence of protocol text in application input.

Confirmed: focus-specific symptoms inside Herdr, no equivalent pause in the user's plain iTerm tab, palette-response text in Claude input, blocking stacks during a reproduced pause, failed redraw-only mitigation, and platform-specific palette-query policy in the release source.

Unconfirmed: the exact parser defect, whether all pause duration is caused by the palette burst, whether the defect is entirely in Herdr or depends on iTerm behavior, whether another release already fixes it, and any successful workaround.
