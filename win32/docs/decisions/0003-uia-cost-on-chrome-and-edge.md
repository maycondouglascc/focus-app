# 0003: Reading the address bar through UI Automation is cheap enough (Chrome, Edge, Firefox)

Measured on 2026-11-16 with `examples/uia_probe` (release build), real browsers
with many tabs open. Each run: 30 s baseline, 60 s reading the address bar once
a second (the monitor will read far less often, only when the title changes),
30 s after. Only numbers and the URL scheme are printed, never titles or URLs.

| Browser | Address bar found | Latency avg / max | Browser CPU baseline / probing / after | Probe process private |
|---|---|---|---|---|
| Edge | 58/58 | 31 ms / 223 ms | 0.0% / 5.8% / 3.0% | 4.3 -> 5.2 MB |
| Chrome | 58/58 | 45 ms / 110 ms | 37.4% / 30.7% / 0.0% | 4.3 -> 5.0 MB |
| Firefox | 54/54 | 117 ms / 198 ms | 1.5% / 8.3% / 2.9% | 4.3 -> 5.3 MB |

Notes:
- Browser memory fell during probing in both runs (tabs being discarded), so
  there is no sign of UIA inflating it.
- Browser CPU is dominated by what the browser was doing anyway: Chrome was
  busier in baseline than while probed. Edge's 5.8% at 1 query/s is an upper
  bound; at one read per title change it will be a small fraction of that.
- Chrome and Firefox return the address without the scheme (`none` above), so
  the monitor must not rely on `https://` being present.
- Firefox is the user's main browser and the slowest to read (117 ms avg, and
  its CPU goes from 1.5% to 8.3% at 1 query/s, the clearest rise of the three).
  At one read per title change that is acceptable. Firefox exposes no Value
  pattern: the address is the Name of the ComboBox's Text child.
- Max latency of 223 ms means the read must stay off the foreground-monitor
  thread.

Decision: pass. Keep URL capture via UI Automation for Chrome and Edge, on its
own thread, read only on title change. The tab-title-only fallback is not
needed.
