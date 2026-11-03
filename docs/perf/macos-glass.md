# macOS on-glass keystroke latency

macOS gives an app no presentation feedback for a `CALayer` contents change, so the shell cannot
measure when a keystroke reaches the glass. It logs an estimate instead, `key_glass_est_ms`:

```
key_glass_est_ms = CADisplayLink.targetTimestamp (at the tick that commits the key)
                 + one refresh period
                 - NSEvent.timestamp
```

`targetTimestamp` is when the frame being prepared at that tick is due on screen. The extra
refresh allows for the window server compositing the committed layer tree into the following
frame. All three times share one clock (`CLOCK_UPTIME_RAW`, the base of `NSEvent.timestamp`,
`CACurrentMediaTime` and `marks::now_ns`). The bench reports the value as an estimate; it is
never compared with the on-glass threshold, which uses `key_glass_ms` only.

The rest of the keystroke is measured, not estimated:

| Value | From | To |
|---|---|---|
| `key_wait_ms` | `NSEvent.timestamp` | the `keyDown:` handler starts |
| `key_draw_ms` | handler start | the change is painted and staged into the back IOSurface |
| (not logged) | staging done | the next display-link tick |
| `key_render_ms` | the tick | `CATransaction` commit and flush returned |
| `key_ms` | `NSEvent.timestamp` | commit and flush returned |

## Optional calibration with a 240 fps camera

The estimate's offset from the real glass time can be checked once per machine and display.

1. Use a phone or camera that records at 240 fps (about 4.2 ms per frame). Lock exposure and turn
   off any stabilisation.
2. Open the fixture: `cargo xtask fixture`, then
   `target/dist/<BUNDLE_NAME>.app/Contents/MacOS/<SLUG> target/fixture/test.md`, with
   `<P>MARKS=/path/to/glass.marks` set so key values are logged.
3. Frame the keyboard and the caret's line in the same shot, close enough that a key cap's travel
   and a glyph appearing are both clear.
4. Press one key at a time, firmly, about once a second, 30 to 50 times.
5. For each press, count frames from the first frame where the key cap reaches the bottom of its
   travel to the first frame where the glyph is visible. Multiply by the frame time.
6. Pair each camera latency with the logged `key_glass_est_ms` for the same press (they arrive in
   order). The key switch fires before the cap bottoms out, so subtract the keyboard's own
   latency if it is known, or report it as a fixed offset.
7. Report the median and p95 of (camera minus estimate) with the display's refresh rate
   (`refresh_hz` in the log). A stable offset within one refresh period means the estimate tracks
   the glass; a spread wider than that means it does not, and the report should say so.

Record the results, the camera, the display and the macOS version in the checkpoint document.
