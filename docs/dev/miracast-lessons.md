# Miracast Bring-up: Lessons, Pitfalls, and Diagnostic Playbook

**Read this before changing the caster.** Most of the time lost in this bring-up
came from treating a plausible protocol exchange as proof, or from changing two
variables at once. This document records the failures, the evidence that
identified them, the fixes that survived hardware, and the diagnostic recipes
that shorten the next investigation.

The architecture and protocol map is in
[`docs/explanation/miracast-source.md`](../explanation/miracast-source.md). The
operational handoff is [`miracast-handoff.md`](miracast-handoff.md). Decisions
are in [ADR-0192](../adr/0192-first-party-miracast-caster.md) and
[ADR-0191](../adr/0191-casting-p2p-belongs-to-the-system-wifi-authority.md).

## The one rule

> **A local decode is not a sink display. A `200 OK` is not a session. A
> `Streaming` state is not a picture. A picture is not audio.**

Every claim needs the right evidence:

| Claim | Minimum evidence |
|-------|------------------|
| The exchange is correct | the real sink accepts `SETUP`/`PLAY` and the source sends frames |
| The video path works | the real sink **displays** the pattern/desktop |
| The audio path works | the real sink **plays** the tone/music |
| A test pattern is not the product | the desktop path is tested separately |
| A capture/mux is valid | `ffprobe`/`ffmpeg` decode the exact capture; a TV outcome still needs the TV |
| A network problem is a caster bug | the live radio/interface combinations and the station link say so |

## Reference environment

| Item | Value |
|------|-------|
| Sink | Skyworth A3F_99E, `stagefright/1.2 (Linux;Android 9)` |
| Sink M4 video | `38 00 01 01 f9cf94a0 00000000 00000000 00 0000 0000 00 none none` |
| Sink M4 audio | `AAC 00000001 00` |
| Sink M4 RTP | `RTP/AVP/UDP;unicast 19000 0 mode=play` |
| P2P | TV is GO `192.168.49.1`; source is a client `192.168.49.x` |
| Sink quirk | exits Screen Mirroring after a failed/ended session; re-enable before every test |
| Sink audio sanity | phone → TV casting **does** play audio |
| Host Wi-Fi | iwd, no NetworkManager/wpa_supplicant; `iw` is installed and used for the concurrency check |

The reference TV's phone-cast audio is the control experiment that proved the
sink's audio path itself is functional. Without that, every source-side audio
experiment is uninterpretable.

## Case index

| ID | Failure |
|----|---------|
| L-001 | Direction inverted: source is the RTSP server |
| L-002 | A sink without a WFD port is still castable |
| L-003 | Session-less `TEARDOWN` is a reset |
| L-004 | The reference sink needs source-driven M1–M6 |
| L-005 | The trigger must be inside M5 |
| L-006 | `SETUP`'s 200 must name the transport |
| L-007 | RTP must leave from the advertised source port |
| L-008 | Never invent `wfd_video_formats` |
| L-009 | The persistent firewall is not the live firewall |
| L-010 | PAT/PMT every second is not enough |
| L-011 | One tiny audio PES per frame is an outlier |
| L-012 | The TS tail packet can underflow |
| L-013 | Our first SDT made the sink worse |
| L-014 | PIDs/PCR aligned, clicks stayed |
| L-015 | The 16 s tone missed the capture start |
| L-016 | The default sink was muted |
| L-017 | Pre-roll/first-group flush reduce startup underruns |
| L-018 | RTCP SR is sync, not audio data |
| L-019 | iwd P2P can fail on HE interoperability |
| L-020 | P2P/station concurrency is driver-dependent (measured) |
| L-021 | Do not capture a phone with monitor mode |
| L-022 | The Control Center must not fabricate state |
| L-023 | Shell quoting creates fake protocol failures |
| L-024 | Low frame rate: measure the layer before tuning |

## Case files

### L-001 — The direction was inverted

- **Symptom:** the caster tried to connect to the TV's 7236; the TV never
  connected back.
- **Root cause:** Miracast makes the **source the RTSP server**. The sink is the
  client. The source listens on 7236 and the sink connects.
- **Fix:** `cast`/`handshake` listen; `sink` is the scripted client. Removed the
  source-dials-sink path, `--host`/`--listen`, and the neighbour-table endpoint
  resolver.
- **Guardrail:** any code that makes the source dial the sink's 7236 is a
  different protocol. The sink's IP arrives with the accepted TCP connection.

### L-002 — A sink without a WFD port is still castable

- **Symptom:** the reference TV advertised `Sink=true` but no WFD `Port`; the
  endpoint resolver rejected it with `NoPort`.
- **Root cause:** in the correct direction the **source** owns the control
  port. The sink does not need to advertise one.
- **Fix:** remove the port requirement; the source advertises its own 7236 and
  waits.
- **Guardrail:** "no RTSP port" is not a castability blocker.

### L-003 — A session-less `TEARDOWN` is a reset, not the end

- **Symptom:** the TV connected, sent `TEARDOWN` with an empty `Session:`, and
  the source closed.
- **Root cause:** the sink was clearing a stale session and expected the source
  to continue.
- **Fix:** answer the `TEARDOWN` and open the exchange; keep the connection.
- **Guardrail:** do not treat every `TEARDOWN` as terminal; inspect the
  `Session` header and the phase.

### L-004 — The reference sink needs the source to drive the exchange

- **Symptom:** after the sink's first `OPTIONS`, the TV waited exactly ~30 s
  and closed; the source was only answering.
- **Root cause:** this Android 9 sink tolerates the documented sink-first order
  but expects the source-driven M3/M5 path in practice.
- **Fix:** `--opening source` (and the `[cast] opening = "source"` default):
  source `OPTIONS` → `GET_PARAMETER` → `SET_PARAMETER`.
- **Guardrail:** keep the permissive responder for documented sinks; the
  source-driven path is a compatibility path, not a replacement for the state
  machine.

### L-005 — The trigger must be inside M5

- **Symptom:** after M5 the TV returned `200`, then closed. A separate source
  M7 `SET_PARAMETER wfd_trigger_method: SETUP` got **no response**; the TV
  closed without acknowledging it.
- **Root cause:** this sink consumes the trigger from the **M5 parameter body**.
- **Fix:** `wfd_trigger_method: SETUP` is part of the M5 body; no source M7.
- **Guardrail:** "the sink accepted M5" and "the sink acted on the trigger" are
  different facts. Do not treat an unanswered M7 as an accepted trigger.

### L-006 — SETUP's 200 must name the transport

- **Symptom:** the TV sent `SETUP`, the source replied 200 with only `Session`,
  the TV played **one frame** and closed.
- **Root cause:** the `SETUP` response lacked `Transport` (and a server port).
- **Fix:** echo the sink's `Transport` and append
  `server_port=19002-19003`; reply also carries `Session: 1`.
- **Guardrail:** `Session` alone is not a transport establishment.

### L-007 — RTP must leave from the advertised source port

- **Symptom:** after the SETUP fix, the source streamed 300 frames; the TV
  accepted the RTSP session but showed no picture. A local TS decode was valid.
- **Root cause:** the RTP sender used an **ephemeral** UDP source port while
  `SETUP` advertised `server_port=19002`. The sink filtered/dropped the packets.
- **Fix:** bind the media socket to the **RTSP local P2P address and port
  19002**; the trace now says
  `media: RTP 192.168.49.170:19002 -> 192.168.49.1:19000`.
- **Guardrail:** the media source port is part of the transport contract. A
  local `ffprobe` of the bytes proves the payload, not delivery.

### L-008 — Never invent `wfd_video_formats`

- **Symptom:** bring-up began with a four-byte placeholder `00 00 02 10` and
  later guessed 720p values; those runs closed after M5. The close was **not
  proven** to be the video bitfield.
- **Root cause of the closes:** the hard blockers were the M5 trigger (L-005),
  the SETUP transport (L-006), and the RTP source port (L-007). Separately,
  `wfd_video_formats` is a packed bitfield; an invalid value can fail
  indistinguishably from those problems.
- **Fix:** use the hardware-validated value
  `38 00 01 01 00000020 00000000 00000000 00 0000 0000 00 none none` as the
  default; for another sink, take its own M4 value. Empty values are rejected
  before the network.
- **Guardrail:** do not blame the bitfield until trigger/Transport/RTP-port and
  audio are excluded. The `formats` tool annotates; it does not solve the sink.

### L-009 — The persistent firewall is not the live firewall

- **Symptom:** the probe reported `BLOCKED` from `/etc/nftables.conf` while the
  TV connected successfully.
- **Root cause:** the persistent file had `policy drop`; the live ruleset
  admitted the P2P traffic. The probe read the wrong authority.
- **Fix:** read only the live ruleset; when it is unreadable, report
  **unverified** and do not borrow the file's verdict. `doctor` names a blocker
  only from live evidence.
- **Guardrail:** do not ask the user to add `nft` rules unless `doctor` proves a
  live block. The TV connecting to 7236 is proof that inbound works.

### L-010 — PAT/PMT every second is not enough

- **Symptom:** the TV played the pattern but the audio had frequent short
  gaps; a local decode was clean.
- **Root cause:** the home-grown TS emitted PAT/PMT once per second; the ffmpeg
  reference emits them every **100 ms**. The 1 s cadence was an outlier, but
  whether PSI cadence alone caused the gaps was not isolated.
- **Fix:** PAT/PMT every 100 ms (and on the first frame). This is the correct
  baseline; later A/B runs still had clicks, so it was necessary but not
  sufficient.
- **Guardrail:** compare PSI cadence against a known-good TS, not just the
  payload; do not claim it fixed audio until the A/B says so.

### L-011 — One tiny audio PES per frame starves some decoders

- **Symptom:** audio clicks/underruns with one AAC frame (≈21 ms) per PES.
- **Root cause:** the ffmpeg reference groups about **8 AAC frames per PES**;
  our per-frame PES was the outlier. Whether grouping alone caused the underruns
  was not isolated.
- **Fix:** group 8 ADTS frames per audio PES, PTS of the first frame. Keep it:
  it matches the reference and reduces the PES rate.
- **Guardrail:** do not revert to per-frame PES without a TV test.

### L-012 — The TS tail packet can underflow

- **Symptom:** generating our own TS panicked with "attempt to subtract with
  overflow".
- **Root cause:** a tail TS packet reserved the adaptation-field flags/length
  *after* choosing the payload; a 183-byte tail left negative stuffing.
- **Fix:** reserve the adaptation overhead before choosing the payload chunk
  (`base_capacity - 2` for a tail; the first PCR packet already reserves 8).
- **Guardrail:** every access unit size must be tested, especially the tail;
  `debug_assert_eq!(packet.len(), 188)` is not optional.

### L-013 — SDT is present in the reference, but our first SDT made it worse

- **Symptom:** adding an SDT (PID `0x11`, 60/30 s) produced **more** frequent
  audio clicks on the reference TV, despite matching the reference table count.
- **Root cause:** not fully identified. The first implementation emitted the SDT
  before PAT/PMT; the exact difference from ffmpeg (content, order, or service
  descriptor) was not established. The SDT is not required for PES decode.
- **Fix / current state:** do **not** emit the SDT in production. The reference
  ffmpeg TS has one; keep it in the comparison corpus. Revisit only with a
  capture and a byte-level diff of the SDT section.
- **Guardrail:** "the reference has it" is not enough; adding a PSI table can
  trigger demuxer resets. One-change-at-a-time and listen.

### L-014 — PIDs and PCR cadence were aligned; the clicks stayed

- **Symptom:** after aligning video/audio PIDs to `0x100/0x101`, PCR to 100 ms,
  audio PES to 176, audio packets to 2815, the TV still clicked.
- **Root cause:** **not isolated.** `ours_low.ts` (our muxer + a single-IDR
  video) and `ref_small.ts` (ffmpeg mux + x264 P-frames) differ in **two**
  variables: the video encoding/bitrate and the muxer (including our absent
  SDT). The click reduction in `ref_small.ts` does not prove which one matters.
- **Lesson:** the all-IDR test pattern is a poor audio test. Validate audio on
  the desktop P-frame path. To isolate the muxer, remux the reference P-frame
  video with our `TsMuxer` and compare; do not declare the video encoding the
  cause from this pair alone.
- **Guardrail:** one variable at a time. `ours_low` vs `ref_small` is not a
  single-variable test.

### L-015 — The 16-second one-shot tone was before the capture started

- **Symptom:** "no audio at all" in live runs; the dumped AAC was `-91 dB`
  (silence), while the same file played fine locally.
- **Root cause:** the test played a 6 s tone at t=16 s; `settle 12` + P2P group
  + RTSP + `PLAY` took longer, so `AacCapture` (started after `PLAY`) missed the
  tone entirely.
- **Fix:** play a **60 s continuous tone from the start**, or start the capture
  before the handshake and pre-roll; the live dump became 488 KB with a
  `-31.6 dB` tone.
- **Guardrail:** when testing capture, the signal must span the capture start.
  A silent capture is not proof of a broken sink or codec.

### L-016 — The default sink was muted

- **Symptom:** local `pw-play` seemed fine, but the source captured silence; the
  `wpctl` sink showed `MUTED`.
- **Root cause:** the monitor of the default sink was muted (or the default sink
  changed).
- **Fix:** `wpctl set-mute @DEFAULT_AUDIO_SINK@ 0`; verify the default sink id
  with `wpctl status`; the source resolves `@DEFAULT_AUDIO_SINK@` at capture
  start.
- **Guardrail:** always check the default sink and mute state before an audio
  test; do not debug AAC while capturing silence.

### L-017 — Pre-roll and first-group flush reduce startup underruns

- **Symptom:** the tone was continuous after the start, but the first two or
  three audio moments had short blanks.
- **Root cause:** the sink's audio decoder started on an empty pipe; the first
  8-frame audio PES arrives ~170 ms after capture start.
- **Fix:** start `AacCapture` **before** the RTSP handshake (small pre-roll) and
  flush the first partial audio group on the first video frame. A 1.5 s
  `MEDIA_START_PTS` lead matches the ffmpeg reference's ~1.4 s start.
- **Guardrail:** startup gaps are different from steady-state gaps. Measure
  them separately; do not "fix" the steady state by changing startup code.

### L-018 — RTCP SR is for sync, not audio data

- **Symptom:** with audio enabled, some runs were silent even after a valid
  handshake; the capture dump showed silence (L-015/L-016). Adding RTCP SR did
  not change that silence.
- **Root cause:** the silence was the capture/timing/mute path, not RTCP. RTCP
  does not carry audio; it provides the NTP/RTP clock relationship, and the
  source was also sending no SR.
- **Fix:** send a compound SR + SDES(CNAME) from 19003 to 19001 every second;
  parse RR if a sink sends one. The reference TV sends **no RR**.
- **Guardrail:** do not use RTCP to explain or fix a silent capture. Missing RR
  is not proof of no packet loss, and a sink that sends no RR cannot be used as
  a loss meter.

### L-019 — iwd P2P can fail on HE interoperability

- **Symptom:** `PushButton` failed 3/3;
  `invalid HE capabilities`, `4-Way handshake failed ... reason: 15`, and
  `iwd.service: Failed with result 'core-dump'`.
- **Root cause:** iwd/driver/TV HE interoperability, not WFD.
- **Fix:** `sudo systemctl restart iwd`; optionally
  `iwlwifi disable_11ax=1` if it recurs. Retest after the restart.
- **Guardrail:** do not misattribute a radio failure to the WFD state machine.

### L-020 — P2P/station concurrency is driver-dependent (measured)

- **Symptom:** while casting, normal Wi-Fi/network connectivity appeared lost.
- **Root cause:** Wi-Fi Direct P2P uses the same physical adapter. The outcome
  depends on the driver's interface combinations and channels, not on the caster.
- **Reference-host evidence:** `iw list` reports
  `managed <= 2`, `P2P-client/GO <= 1`, `P2P-device <= 1`, `#channels <= 2`.
  In a live 60-frame P2P session the P2P ran on channel **11** while the station
  stayed on channel **6**. During the **~6 s group formation window** the station
  was disconnected: AP/internet pings failed and existing TCP sessions could
  stall. After the station reassociated, AP/internet pings succeeded for the
  rest of the session. The teardown transition was not separately measured.
  This outage is a limitation of the tested **Linux/iwd/driver group-formation
  path**, not proof that the hardware lacks steady-state concurrency; vendor
  stacks often hide the transition or use a dedicated P2P radio.
- **Fix:** if a brief, real outage is acceptable, the single radio works in the
  steady state. For uninterrupted networking, use **Ethernet**, a **second Wi-Fi
  adapter** dedicated to P2P, or a second machine. Check `iw list` and the actual
  P2P/AP channels on the target adapter; do not assume this matrix applies
  everywhere.
- **Guardrail:** measure both the transition and the steady state. ADR-0191
  assigns P2P to the system Wi-Fi authority; the caster does not own the station.

### L-021 — Do not capture a phone with monitor mode

- **Symptom / question:** "how do I capture the phone's Wi-Fi Direct stream from
  the laptop?"
- **Root cause:** phone↔TV is a WPA2 P2P group; the laptop is not in that group.
  Monitor mode sees encrypted unicast frames; it cannot decrypt them without
  the group key.
- **Fix:** capture on a **source** device (a rooted Android with `tcpdump`; a
  Windows/Linux source with Wireshark/tcpdump), use ADB/root on the TV if
  available, or use a Linux live USB with GNOME Network Displays as a
  known-good source. A no-root phone can still answer the **phone-cast audio**
  question without a capture.
- **Guardrail:** do not spend time on monitor-mode captures of a WPA2 P2P group.

### L-022 — The Control Center must not fabricate state

- **Symptom:** the old UI set `connected=true`, `cast_streams=1`, and spawned a
  caster without the required formats; `pkill -f` stopped sessions.
- **Root cause:** the compositor guessed instead of consuming the service.
- **Fix:** `tessera-castd` owns discovery/sessions; `CastStatus` is the source of
  truth; Connect only when the service is available and the blocker is `None`;
  Stop uses the service session id; no `pkill`.
- **Guardrail:** no fake `connected`, no process-name lifecycle, no compositor
  iwd/P2P ownership.

### L-023 — Shell quoting creates fake protocol failures

- **Symptom:** `--opening needs a value`, `-bash: source: --: invalid option`,
  `--timeout needs a value`, and `$VF` not expanding.
- **Root cause:** line breaks split an option from its value; an environment
  assignment on the same command does not expand for that command.
- **Fix:** use a single line or correct `\` continuations; assign `VF=...` on
  its own line; do not use `VF=... ./binary --video-formats "$VF"` in one
  command.
- **Guardrail:** validate the exact command before diagnosing the protocol.

### L-024 — Low frame rate: measure the layer before tuning

- **Symptom:** eye-visible low FPS while casting.
- **First checks:** actual source FPS = `frames_sent / duration`; the running
  `[cast] fps` / `--fps` (the diagnostics used 10; the default is 30); restart
  the daemon after a config change. Test the **pattern** at 30 and the
  **desktop** separately; if the pattern is 30 and the desktop is not, the
  bottleneck is desktop capture/VA-API, not the radio.
- **Radio:** the station and P2P share airtime even with two channels. Move the
  station to **Ethernet** or a second adapter; this also removes the ~6 s
  formation outage. Check the P2P channel/link (`iw dev`, `station dump`): 2.4
  GHz ch11 is the reference TV's GO; a 5 GHz P2P or a dedicated adapter can
  improve the link.
- **Then:** check the desktop portal/PipeWire/VA-API (pattern vs desktop,
  encoder bitrate/resolution), and compare the same content on a phone. Do not
  tune the caster's `fps` while the source is still on a 10 fps diagnostic, and
  do not blame the TV before the phone baseline.

## Diagnostic playbook

### Local, no hardware

```bash
# protocol and media unit tests
cargo test -p tessera-cast

# real source ↔ scripted sink over a socketpair/loopback
# (the test suite does this; for a manual run use two shells)
./target/debug/tessera-cast cast --source pattern --video-formats "<value>"
./target/debug/tessera-cast sink --connect 127.0.0.1:7236 --play

# inspect a TS/AAC
ffprobe -show_streams /tmp/ref.ts
ffmpeg -i /tmp/ref.ts -map 0:a -af volumedetect -f null -
ffmpeg -i /tmp/live.aac -af "silencedetect=noise=-50dB:d=0.05" -f null -
```

### A/B against a known-good muxer

Generate a reference TS with ffmpeg, then use the `--ts-file` diagnostic to send
that exact TS through the real RTSP/P2P/RTP/RTCP path:

```bash
ffmpeg -loop 1 -i /tmp/pattern.png -f lavfi -i "sine=frequency=660:duration=30" \
  -t 30 -c:v libx264 -profile:v baseline -pix_fmt yuv420p -r 30 -g 30 \
  -c:a aac -ar 48000 -ac 2 -b:a 128k -f mpegts /tmp/ref.ts

./target/debug/tessera-cast cast --peer 创维 --opening source \
  --video-formats "38 00 01 01 00000020 00000000 00000000 00 0000 0000 00 none none" \
  --ts-file /tmp/ref.ts --ts-seconds 30
```

- `ref.ts` continuous, `ours.ts` clicking → inspect the muxer/TS.
- both clicking → video/encoder/bitrate or the sink, not the audio capture.
- `--ts-file` keeps the handshake, P2P, RTP, RTCP, and pacing identical; that is
  the point.

The bring-up tree also accepts `TESSERA_CAST_DUMP_AUDIO=/path/aac` and
`TESSERA_CAST_DUMP_TS=/path/ts` for one live session. These are diagnostic
switches, not product features; remove or gate them once the media path is
settled.

### Live hardware

```bash
# The TV must be in Screen Mirroring; it exits after each session.
./target/debug/tessera-cast doctor
./target/debug/tessera-cast cast --peer 创维 --opening source \
  --source pattern --video-formats "<value>" \
  --audio --fps 10 --max-frames 300 --timeout 30
```

- Use a **60 s continuous tone** or start the tone after `sink accepted`; a
  16 s one-shot can miss the capture start.
- `wpctl status` / `wpctl set-mute @DEFAULT_AUDIO_SINK@ 0`; the default sink must
  not be muted.
- The trace (`SessionEvent::Trace`) contains the exact M1–M6/SETUP/PLAY bytes.
- `media: RTP <local>:19002 -> <sink>:19000` confirms the source port.
- `rtcp rr:` lines appear only if the sink sends receiver reports; the reference
  TV does not.
- The TV's RTSP/session may end with `reached frame limit`; that is normal.

### Radio and network

```bash
wpctl status
systemctl restart iwd         # only when P2P/HE fails; user action
iw list | sed -n '/valid interface combinations/,/^$/p'
iw dev
```

There is no `iw` on the reference shell; install it before claiming a
concurrency diagnosis. A second Wi-Fi adapter or Ethernet is the robust fix.

### Capturing a known-good phone source

- Rooted Android: push a static `tcpdump`, capture on `any`/`p2p-wlan0-0`
  while casting, pull the pcap.
- Windows PC as the source: Wireshark on the Microsoft Wi-Fi Direct Virtual
  Adapter; filter `tcp.port == 7236 || (udp.port >= 19000 && udp.port <= 19010)`.
- Linux known-good source: GNOME Network Displays (or `gst-rtsp-server-wfd`)
  plus `tcpdump` on the local interface.
- No-root phone: PCAPdroid may not see system WFD media; do not rely on it.
  The phone-cast audio test still answers whether the TV's sink audio works.
- Monitor mode on the laptop is not a solution for a WPA2 P2P group.

## Do-not-regress checklist

- [ ] Source is the RTSP server; sink connects; the source never dials 7236.
- [ ] A sink without a WFD port is castable.
- [ ] Session-less `TEARDOWN` is answered and the exchange continues.
- [ ] Source-driven M1–M6 is available for the reference Android sink.
- [ ] The SETUP trigger is in the **M5 body**, not a separate M7.
- [ ] `SETUP` 200 carries `Session` **and** `Transport` with `server_port`.
- [ ] RTP is sent from the P2P local IP and the advertised **19002**.
- [ ] `wfd_video_formats` is never invented; empty values are rejected.
- [ ] PAT/PMT every 100 ms; audio PES groups 8 AAC frames.
- [ ] TS tail adaptation reserves its overhead; every packet is 188 bytes.
- [ ] RTCP SR is sent from 19003 to 19001; RTCP RR is parsed if present.
- [ ] Audio is opt-in; the sink's AAC is echoed; no audio claim without
      `AudioActive`.
- [ ] Audio capture resolves the default sink; mute/volume is a test precondition.
- [ ] Startup tests use a signal that spans the capture start, not a one-shot.
- [ ] The test pattern's all-IDR video is not used to judge the desktop audio.
- [ ] Firewall verdicts use the live ruleset; otherwise `unverified`.
- [ ] No fake `connected`, no `pkill`, no compositor P2P/iwd ownership.
- [ ] The TV is re-enabled after each failed session.
- [ ] A/B one variable at a time; keep the exact commands and evidence.

## Open questions

1. **Audio clicks on the test pattern.** `ref.ts`/`ref_small.ts` are continuous
   or near-continuous; `ours_low.ts` (single-IDR video) is worse. The remaining
   few clicks appear tied to video content/encoder and the sink's decoder, not
   to AAC/TS/RTP. Validate the desktop VA-API P-frame path and, if needed,
   replace the all-IDR pattern asset with a P-frame sequence.
2. **Control Center audio.** The code/config supports it (`[cast] audio = true`),
   but the default is off and the reference sink was muted during one test.
   Confirm the UI path and show `CastSession.audio`.
3. **STA + P2P concurrency.** The host's driver combinations are not yet
   recorded because `iw` is absent. Confirm `managed` + `P2P` and `#channels`;
   decide between Ethernet, a second adapter, or an adapter-selection config.
4. **Frame rate.** Recent diagnostics used `--fps 10`; the default is 30.
   Measure the desktop at 30 fps separately from the pattern test.
5. **SDT.** The reference TS has one; our first SDT worsened the sink. Revisit
   with a capture, not from the table name alone.

## References

- [Miracast source architecture](../explanation/miracast-source.md)
- [Miracast handoff](miracast-handoff.md)
- [ADR-0192](../adr/0192-first-party-miracast-caster.md)
- [ADR-0191](../adr/0191-casting-p2p-belongs-to-the-system-wifi-authority.md)
- [Wi-Fi Alliance Miracast Specification v2.3](https://www.wi-fi.org/system/files/Miracast_Specification_v2.3.pdf)
- [Wi-Fi Display Technical Specification v2.1 (mirror)](https://raw.githubusercontent.com/wiki/albfan/miraclecast/files/Wi-Fi_Display_Technical_Specification_v2.1_0.pdf)
- [RTSP RFC 2326](https://www.rfc-editor.org/rfc/rfc2326)
- [RTP RFC 3550](https://www.rfc-editor.org/rfc/rfc3550)
- [RTP MPEG2-TS RFC 2250](https://www.rfc-editor.org/rfc/rfc2250)
- [RTP payload types RFC 3551](https://www.rfc-editor.org/rfc/rfc3551)
- [ITU-T H.222.0 / MPEG2-TS](https://www.itu.int/rec/T-REC-H.222.0)
- [ITU-T H.264](https://www.itu.int/rec/T-REC-H.264)
- [iwd](https://iwd.wiki.kernel.org/)
- [PipeWire](https://docs.pipewire.org/)
- [GNOME Network Displays](https://gitlab.gnome.org/GNOME/gnome-network-displays)
- [gst-rtsp-server-wfd](https://github.com/albfan/gst-rtsp-server-wfd)
- [miraclecast](https://github.com/albfan/miraclecast)
