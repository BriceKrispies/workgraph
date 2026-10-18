import { expect, test } from "@playwright/test";

const base = () => process.env.WG_E2E_BASE_URL ?? "";

test.beforeEach(() => {
  test.skip(!!process.env.WG_E2E_SKIP, process.env.WG_E2E_SKIP);
});

test("overview shows real persisted counts", async ({ page, request }) => {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(`${base()}/`);
  const html = await (await request.get(`${base()}/`)).text();
  const token = /name="workgraph-token" content="([^"]+)"/.exec(html)?.[1];
  expect(token).toBeTruthy();
  const workspaces = await (await request.get(`${base()}/api/v1/workspaces`, { headers: { Authorization: `Bearer ${token}` } })).json();
  const summary = await (
    await request.get(`${base()}/api/v1/workspaces/${workspaces[0].slug}`, { headers: { Authorization: `Bearer ${token}` } })
  ).json();
  const total = Object.values(summary.tasks_by_status as Record<string, number>).reduce((a, b) => a + b, 0);
  expect(total).toBeGreaterThan(0);
  await expect(page.getByTestId("stat-Tasks")).toHaveText(String(total));
  await expect(page.getByTestId("stat-Repositories")).toHaveText(String(summary.repositories));
  await expect(page.getByTestId("recent-activity")).toBeVisible();
  expect(errors).toEqual([]);
});

test("tasks list and task detail show commits and attempts", async ({ page }) => {
  await page.goto(`${base()}/tasks`);
  const table = page.getByTestId("tasks-table");
  await expect(table).toBeVisible();
  const rows = table.locator("tbody tr");
  expect(await rows.count()).toBeGreaterThan(0);
  // Open an accepted task: it has linked commits and attempts.
  await page.getByLabel("Status").selectOption("accepted");
  await expect(rows.first()).toBeVisible();
  await rows.first().locator("a").first().click();
  await expect(page.getByTestId("history")).toBeVisible();
  await page.getByRole("tab", { name: "Commits & changes" }).click();
  await expect(page.getByTestId("task-commits")).toBeVisible();
  expect(await page.getByTestId("task-commits").locator("tbody tr").count()).toBeGreaterThan(0);
  await expect(page.getByText("Cumulative churn")).toBeVisible();
  await page.getByRole("tab", { name: "Attempts" }).click();
  await expect(page.getByTestId("task-attempts")).toBeVisible();
  await page.getByRole("tab", { name: "Verification" }).click();
  await expect(page.getByText(/run [0-9a-f]{8}/).first()).toBeVisible();
});

test("graph, analytics and automation pages render data", async ({ page }) => {
  await page.goto(`${base()}/graph`);
  await expect(page.getByTestId("task-graph")).toBeVisible();
  expect(await page.locator(".react-flow__node").count()).toBeGreaterThan(0);

  await page.goto(`${base()}/analytics`);
  await expect(page.getByText("Tasks per week")).toBeVisible();
  await expect(page.locator(".recharts-surface").first()).toBeVisible();
  await page.getByRole("tab", { name: "Agents & models" }).click();
  await expect(page.getByTestId("agent-comparison")).toBeVisible();
  await page.getByRole("tab", { name: "Coupling" }).click();
  await expect(page.getByTestId("coupling-matrix")).toBeVisible();

  await page.goto(`${base()}/automation`);
  await expect(page.getByRole("tab", { name: "Rules" })).toBeVisible();
  await page.getByRole("tab", { name: "Jobs" }).click();
  await expect(page.getByRole("button", { name: /^all/ })).toBeVisible();

  await page.goto(`${base()}/execution`);
  await expect(page.getByTestId("attempts")).toBeVisible();
  await page.getByTestId("attempts").locator("tbody tr").first().click();
  await expect(page.getByTestId("attempt-progress")).toBeVisible();

  await page.goto(`${base()}/repositories`);
  await expect(page.getByTestId("repositories")).toBeVisible();
  await page.getByTestId("repositories").locator("tbody tr a").first().click();
  await expect(page.getByTestId("repo-commits")).toBeVisible();

  await page.goto(`${base()}/settings`);
  await page.getByRole("tab", { name: "Agent adapters" }).click();
  await expect(page.getByTestId("agents")).toBeVisible();
});

test("stacks: a stack created through the API shows on the stacks pages", async ({ page, request }) => {
  await page.goto(`${base()}/`);
  const html = await (await request.get(`${base()}/`)).text();
  const token = /name="workgraph-token" content="([^"]+)"/.exec(html)?.[1];
  expect(token).toBeTruthy();
  const headers = { Authorization: `Bearer ${token}` };
  const workspaces = await (await request.get(`${base()}/api/v1/workspaces`, { headers })).json();
  const slug = workspaces[0].slug;
  const tasks = await (await request.get(`${base()}/api/v1/ws/${slug}/tasks`, { headers, params: { limit: 200 } })).json();
  const parent = (tasks.items as { key: string; title: string }[]).find((t) => t.title === "Session reliability initiative");
  expect(parent).toBeTruthy();
  const created = await request.post(`${base()}/api/v1/ws/${slug}/stacks`, { headers, data: { parent: parent!.key } });
  expect(created.status()).toBe(201);

  await page.goto(`${base()}/stacks`);
  const table = page.getByTestId("stacks");
  await expect(table).toBeVisible();
  await expect(table.getByTestId("stack-row").filter({ hasText: parent!.key })).toBeVisible();
  await table.getByRole("link", { name: new RegExp(parent!.key) }).click();
  await expect(page.getByTestId("stack-repositories")).toBeVisible();
  await expect(page.getByTestId("stack-items").getByTestId("stack-item").first()).toBeVisible();
  await expect(page.getByTestId("stack-timeline")).toContainText("Stack created");
});
