#!/usr/bin/env python3
"""Rough paint/jank probe for the mini app (dev only)."""
import asyncio, json, sys
from playwright.async_api import async_playwright

URL = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:8080/"

PROBE = """
() => new Promise((resolve) => {
  const long = [];
  let frames = 0, worst = 0, last = performance.now(), t0 = performance.now();
  const po = new PerformanceObserver((l) => { for (const e of l.getEntries()) long.push(Math.round(e.duration)); });
  try { po.observe({ entryTypes: ['longtask'] }); } catch (e) {}
  function tick(t) {
    frames++;
    if (t - last > worst) worst = t - last;
    last = t;
    if (t - t0 < 3000) return requestAnimationFrame(tick);
    po.disconnect();
    const res = performance.getEntriesByType('resource');
    resolve({
      fps: Math.round(frames / ((t - t0) / 1000)),
      worstFrameMs: Math.round(worst),
      longTasks: long.length,
      worstLongTask: long.length ? Math.max(...long) : 0,
      domNodes: document.getElementsByTagName('*').length,
      resources: res.length,
      transferKB: Math.round(res.reduce((a, r) => a + (r.transferSize || 0), 0) / 1024),
      heapMB: performance.memory ? Math.round(performance.memory.usedJSHeapSize / 1048576) : -1,
    });
  }
  requestAnimationFrame(tick);
})
"""


async def main():
    async with async_playwright() as p:
        b = await p.chromium.launch(args=["--no-sandbox"])
        pg = await b.new_page(viewport={"width": 430, "height": 900}, device_scale_factor=2)
        errs = []
        pg.on("console", lambda m: errs.append(m.text) if m.type == "error" else None)
        pg.on("pageerror", lambda e: errs.append(str(e)))
        await pg.goto(URL, wait_until="domcontentloaded")
        await pg.wait_for_timeout(2500)
        # idle cost
        idle = await pg.evaluate(PROBE)
        # switching tabs is the most common interaction
        for tab in ["members", "queue", "chat", "song"]:
            try:
                await pg.click(f"[data-tab='{tab}']", timeout=2000)
            except Exception:
                pass
            await pg.wait_for_timeout(400)
        after = await pg.evaluate(PROBE)
        print(json.dumps({"idle": idle, "afterTabs": after, "errors": errs[:6]}, indent=1))
        await b.close()


asyncio.run(main())
