# Spike S6: tab video (CDP screencast → ffmpeg) + recorder binding

Date: 2026-11-06 · Windows 11, 12 cores, CEF 156 (debug build) · ffmpeg 8.0 (gyan.dev full
build, installed through scoop) · code: `crates/record/src/video.rs`, `human.rs`.

**Decision: go.** Video v1 = `Page.startScreencast` (JPEG) → `ffmpeg` sidecar through stdin
(`-f image2pipe -c:v mjpeg`), constant frame rate made by us, default **WebM/VP9 at 10 fps**.
MP4 uses `libx264` when the ffmpeg build has it, else `h264_mf` (Windows Media Foundation).
No ffmpeg → `record.video_start` fails with `NOT_FOUND` and the hint
`winget install Gyan.FFmpeg` (or put `ffmpeg.exe` next to `browsermcp.exe`; `BMCP_FFMPEG`
overrides the path).

## Pipeline

```text
Page.startScreencast {format: jpeg, quality: 80, everyNthFrame: 1}
Page.captureScreenshot (jpeg)      → first frame at once (a still page sends no screencast frame)
Page.screencastFrame {data, sessionId}
   └─ keep as "latest frame"; ack (Page.screencastFrameAck) on our next clock tick
clock every 1/fps s (tokio interval, missed ticks caught up) → write latest frame to ffmpeg stdin
stop: ack the rest, Page.stopScreencast, pad missing ticks with the last frame, close stdin, wait
ffmpeg -f image2pipe -framerate F -c:v mjpeg -i - \
       -vf scale=W:H:force_original_aspect_ratio=decrease,pad=W:H:(ow-iw)/2:(oh-ih)/2,format=yuv420p \
       -c:v libvpx-vp9 -deadline realtime -cpu-used 8 -row-mt 1 -b:v 0 -crf 23   out.webm
       (mp4: -c:v libx264 -preset veryfast -crf 23 -movflags +faststart)
```

* **Constant frame rate without timestamps.** Chromium only sends frames while the page paints.
  We repeat the newest frame on a fixed clock, so the file is CFR by construction (`-framerate`
  on the input; no `-vsync`/`-fps_mode` needed). If the machine falls behind, the missing ticks
  are written at stop (at most 10 s), so the video is as long as the recording.
* **W×H** come from the first frame's JPEG header (rounded down to even numbers); later frames of
  another size (window resized) are scaled and padded to it, so ffmpeg never sees a size change.
* **Deferred acks.** Acking each frame at once lets Chromium send at the display rate
  (60–100 Hz here) and JPEG-encode every frame. Acking on our clock tick caps it at about
  2–4× our fps and **halves the browser's CPU** (table below).
* The encoder is spawned with the first frame, `kill_on_drop`, stderr collected for errors.
  Scoop's `ffmpeg.exe` is a launcher ("shim"); we read `ffmpeg.shim` and run the real exe so it
  is our direct child.

## Numbers

Page: `anim.html` (a box moving with CSS animation + a counter updated every 100 ms), window
1264×680. CPU = process CPU time / wall time (100 % = one core), sampled with
`Get-Process`. **The machine was at ~95–100 % total CPU during all runs** (other builds running
in parallel), so wall-clock timings are pessimistic; CPU seconds are the reliable figure.

| Scenario | source frames/s | browser CPU | ffmpeg CPU | output | file (≈11 s) | stop latency |
|---|---|---|---|---|---|---|
| no video | – | 38–43 % | – | – | – | – |
| webm 10 fps, ack at once | 99.8 | 207 % | 13 % | 115 frames, 10/1 | 129 KB | 0.7 s |
| mp4 (x264) 10 fps, ack at once | 96.2 | 210 % | 14 % | 108 frames | 58 KB | 0.4 s |
| **webm 10 fps, deferred ack** | **23.7** | **105 %** | 12 % | 117 frames, 10/1 | 112 KB | 2.4 s |
| webm 30 fps, deferred ack | 38.9 | 125 % | 12 % | 333 frames, 30/1 | 138 KB | 3.7 s |
| mp4 (x264) 10 fps, deferred ack | 20.2 | 108 % | 11 % | 115 frames | 48 KB | 0.7 s |
| still page (login form), webm 10 fps | 0.2 | 1.3 % | 9.5 % | 106 frames (2 real + repeats) | 20 KB | 1.4 s |

* Output frame counts match `wall × fps` (ffprobe `nb_read_frames`, `avg_frame_rate`), i.e. CFR
  works with both a busy and a still page.
* ffmpeg peak working set ≈ 250 MB (VP9 with `-row-mt`); x264 similar.
* Offline (100 frames 1264×680 piped from a file): CPU per 10 s of video — VP9 2.9 s, x264
  1.8 s, h264_mf 1.6 s. All three are faster than real time on one core.
* Stop latency is ffmpeg finishing its queue; it was 0.4–3.7 s under full machine load.
* The login recording (`record.start {video: true}`): 9.5 s, 42 KB WebM.

Choice: **WebM/VP9 10 fps** as default (open format, smallest of the free encoders at equal
quality for UI content, plays in the browser itself); mp4 for sharing. Hardware encoders
(`h264_nvenc`, `h264_qsv`, `h264_amf`) are in this ffmpeg build but not used: availability
varies per machine and CPU cost is already low; revisit for 30–60 fps.

## Recorder binding without `Runtime.enable` (probe on CEF 156)

The action recorder (`record.start`) runs in an isolated world and reports through a CDP
binding. Raw CDP probe results, Runtime domain **not** enabled:

| Probe | Result |
|---|---|
| `Runtime.addBinding {name}` (global) + call in the main world | `Runtime.bindingCalled` **arrives** (no enable needed) — but the function is visible to the page |
| `Runtime.addBinding {name, executionContextName: "<world>"}`, then `Page.createIsolatedWorld` | binding **not installed** in the world (`ReferenceError`); same with Runtime enabled afterwards |
| `Runtime.addBinding {name, executionContextId: <isolated world>}` | function exists **only** in that world (page: `typeof` → `undefined`), `bindingCalled` arrives |
| `Page.createIsolatedWorld` twice with the same name | returns the same context id (same world) |

So the recorder binds by `executionContextId` and re-binds after every main-frame
`Page.frameNavigated` (new document → new world context); events the script produced before
the binding existed are queued in the world and drained once. If no `bindingCalled` arrives
within 1.5 s, it falls back to polling the queue with `Runtime.evaluate {contextId}` every
250 ms (`meta.human_capture: "poll"`). In the live login test capture was `binding`.

## Gaps / later

* Frames only while the tab is painted: a hidden/occluded window stops the screencast (we run
  with `--disable-features=CalculateNativeWinOcclusion --disable-backgrounding-occluded-windows`
  in tests). OSR/headless video is a later phase.
* Audio is not recorded (screencast is video only).
* No pause/resume; one video per `record.video_start`.
