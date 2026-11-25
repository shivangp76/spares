// Workloads for SPARES_BROWSING_BENCH in local_copy.ts.
import type { Database } from "bun:sqlite";
import type { Page } from "playwright-core";
import { parseNoteId } from "../src/utils";

export function seedBrowsing(db: Database) {
  db.exec(`BEGIN;
    INSERT INTO parser(name) VALUES ('typst');
    WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<100)
    INSERT INTO note(id,data,custom_data,parser_id)
      SELECT i,'Keyword owner','{}',(SELECT id FROM parser WHERE name='typst') FROM n;
    WITH RECURSIVE n(i) AS (VALUES(0) UNION ALL SELECT i+1 FROM n WHERE i<9999)
    INSERT INTO note_keyword(note_id,keyword,embedded) SELECT 1+i/100,'Keyword '||i,0 FROM n;`);
  const data = Array.from(
    { length: 1500 },
    (_, i) =>
      `Type ordinary prose here. #key[Keyword ${i}] More text after the keyword.`,
  ).join("\n");
  db.query(
    "INSERT INTO note(id,data,custom_data,parser_id) VALUES (?,?,'{}',(SELECT id FROM parser WHERE name='typst'))",
  ).run(parseNoteId("aaa"), data);
  db.exec(`INSERT INTO card(id,note_id,"order",back_type,due,stability,difficulty,desired_retention,state,custom_data)
    VALUES (1,1,1,2,unixepoch(),1,1,0.9,2,'{}'); COMMIT`);
}

export async function timeEditor(page: Page, origin: string) {
  await page.goto(origin + "/notes?note=aaa");
  const editor = page
    .locator(".cm-content")
    .filter({ has: page.locator(".cm-duplicate-keyword") })
    .first();
  await editor.waitFor();
  await editor.locator(".cm-line").first().click();
  await page.keyboard.press("Home");
  await page.keyboard.press("ArrowRight");
  // Start just after a frame, so the keyboard event arrives at a consistent point in the frame.
  const times: number[] = [];
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("Performance.enable");
  const cpu = async () =>
    (await cdp.send("Performance.getMetrics")).metrics.find(
      (m) => m.name === "TaskDuration",
    )!.value;
  await cdp.send("Emulation.setCPUThrottlingRate", { rate: 4 });
  const tasks: number[] = [];
  try {
    for (let i = 0; i < 65; i++) {
      await page.evaluate(
        () =>
          new Promise<void>((ready) =>
            requestAnimationFrame(() => {
              document.addEventListener(
                "keydown",
                () => {
                  const start = performance.now();
                  requestAnimationFrame(() => {
                    document.documentElement.dataset.keyFrame = String(
                      performance.now() - start,
                    );
                  });
                },
                { once: true, capture: true },
              );
              delete document.documentElement.dataset.keyFrame;
              ready();
            }),
          ),
      );
      const start = await cpu();
      await page.keyboard.press("x");
      await page.waitForFunction(
        () => document.documentElement.dataset.keyFrame !== undefined,
      );
      const elapsed = ((await cpu()) - start) * 1000;
      if (i >= 5) {
        times.push(
          Number(await page.locator("html").getAttribute("data-key-frame")),
        );
        tasks.push(elapsed);
      }
    }
    if (
      !(await editor.locator(".cm-line").first().innerText()).includes(
        "x".repeat(65),
      )
    )
      throw new Error("The test keystrokes did not reach the editor");
  } finally {
    await cdp.send("Emulation.setCPUThrottlingRate", { rate: 1 });
    await cdp.detach();
  }
  console.log("editor key-to-frame ms", JSON.stringify(times));
  console.log("editor main-thread task ms", JSON.stringify(tasks));
}

export async function forecastRequests(
  page: Page,
  origin: string,
  local: boolean,
) {
  await page.goto(origin + "/statistics");
  await page.getByRole("heading", { name: "Forecast", exact: true }).waitFor();
  await page
    .getByText("Loading…", { exact: true })
    .first()
    .waitFor({ state: "hidden" });
  // Capture at the actual transports: HTTP on the host, worker RPC on the device.
  await page.evaluate(() => {
    const paths: string[] = [];
    const record = (path: string) => {
      const name = path.split("?")[0];
      if (
        [
          "/api/review/history",
          "/api/review/statistics",
          "/api/statistics/graphs",
        ].includes(name)
      )
        paths.push(name);
      document.documentElement.dataset.requests = JSON.stringify(paths);
    };
    Worker.prototype.postMessage = new Proxy(Worker.prototype.postMessage, {
      apply(send, worker, args) {
        const [message] = args;
        if (message.op === "handle") record(message.args[1]);
        return Reflect.apply(send, worker, args);
      },
    });
    const fetch = window.fetch;
    window.fetch = (...args) => {
      const url = args[0] instanceof Request ? args[0].url : String(args[0]);
      record(new URL(url, location.href).pathname);
      return fetch(...args);
    };
  });
  for (const days of ["90", "7", "365"]) {
    await page.getByRole("combobox", { name: /^Forecast/ }).selectOption(days);
    await page
      .getByText(new RegExp("due in the next " + days + " days"))
      .waitFor();
    await page
      .getByText("Loading…", { exact: true })
      .first()
      .waitFor({ state: "hidden" });
  }
  console.log(
    local ? "offline forecast requests" : "hosted forecast requests",
    await page.locator("html").getAttribute("data-requests"),
  );
}
