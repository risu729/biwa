import assert from "node:assert/strict";
import { mkdtemp, mkdir, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { test } from "node:test";

import { prepare, apply, createApi, validateSource, targets } from "./hostname-maintenance.mjs";

const revision = "a".repeat(40);
const domainTarget = {
	kind: "domain",
	worker: "example",
	hostname: "example.test",
	zoneId: "zone",
};
const routeTarget = {
	kind: "route",
	worker: "docs",
	patterns: ["example.test/docs", "example.test/docs/*"],
	zoneId: "zone",
};
const attachment = {
	id: "domain-1",
	hostname: "example.test",
	service: "example",
	zone_id: "zone",
	environment: "production",
	enabled: true,
	previews_enabled: false,
};
function fake({ domains = [], routes = [], dns = [] } = {}) {
	const state = {
		domains: structuredClone(domains),
		routes: structuredClone(routes),
		dns,
		writes: [],
	};
	const api = async (method, path, body) => {
		if (method === "GET") {
			if (path.endsWith("/schedules")) return { result: [{ cron: "0 0 * * *" }] };
			if (path.endsWith("/subdomain"))
				return { result: { enabled: false, previews_enabled: false } };
			if (path.includes("/workers/domains?"))
				return {
					result: structuredClone(state.domains),
					result_info: { page: 1, per_page: 100, total_count: state.domains.length },
				};
			if (path.includes("/dns_records?"))
				return { result: state.dns, result_info: { total_count: state.dns.length } };
			if (path.endsWith("/workers/routes")) return { result: structuredClone(state.routes) };
		}
		state.writes.push({ method, path, body });
		if (method === "PUT" && path.endsWith("/workers/domains")) {
			state.domains.push({ ...body, environment: "production", id: "domain-new" });
			return { result: state.domains.at(-1) };
		}
		if (method === "POST" && path.endsWith("/workers/routes")) {
			state.routes.push({ ...body, id: `route-${state.routes.length}` });
			return { result: state.routes.at(-1) };
		}
		throw new Error(`Unexpected API call: ${method} ${path}`);
	};
	return { state, api };
}

test("source declarations match every reviewed target without executing config", async () => {
	await Promise.all(Object.values(targets).map((target) => validateSource(target)));
});
test("source guard rejects an unreviewed hostname", async () => {
	const root = await mkdtemp(`${tmpdir()}/hostname-test-`);
	try {
		await mkdir(`${root}/config`);
		await writeFile(
			`${root}/config/cloudflare.config.ts`,
			'import { defineConfig } from "cf/config"; export default defineConfig({worker: {name: "example", domains: ["other.test"]}});',
		);
		await assert.rejects(
			validateSource({ ...domainTarget, config: "config/cloudflare.config.ts" }, root),
			/allowlist/u,
		);
	} finally {
		await rm(root, { recursive: true, force: true });
	}
});
test("matching domain plan and approved apply are both no-ops", async () => {
	const { api, state } = fake({ domains: [attachment] });
	const plan = await prepare(api, domainTarget, revision);
	assert.deepEqual(plan.changes, []);
	await apply(api, plan, plan.digest);
	assert.deepEqual(state.writes, []);
});
test("domain plan is read-only and includes one explicit attachment", async () => {
	const { api, state } = fake();
	const plan = await prepare(api, domainTarget, revision);
	assert.equal(plan.changes.length, 1);
	assert.equal(plan.changes[0].method, "PUT");
	assert.deepEqual(state.writes, []);
});
test("existing DNS blocks even initial domain attachment", async () => {
	const { api, state } = fake({ dns: [{ type: "AAAA", content: "100::" }] });
	await assert.rejects(prepare(api, domainTarget, revision), /Existing.*DNS/u);
	assert.deepEqual(state.writes, []);
});
test("a domain owned by another Worker cannot be taken over", async () => {
	const { api, state } = fake({ domains: [{ ...attachment, service: "other" }] });
	await assert.rejects(prepare(api, domainTarget, revision), /conflicting/u);
	assert.deepEqual(state.writes, []);
});
test("wrong confirmation digest prevents every write", async () => {
	const { api, state } = fake();
	const plan = await prepare(api, domainTarget, revision);
	await assert.rejects(apply(api, plan, "b".repeat(64)), /reviewed digest/u);
	assert.deepEqual(state.writes, []);
});
test("drift after plan invalidates apply before mutation", async () => {
	const { api, state } = fake();
	const plan = await prepare(api, domainTarget, revision);
	state.domains.push({ ...attachment, id: "elsewhere", hostname: "other.test" });
	await assert.rejects(apply(api, plan, plan.digest), /changed after planning/u);
	assert.deepEqual(state.writes, []);
});
test("domain apply uses only the public attachment endpoint and verifies readback", async () => {
	const { api, state } = fake();
	const plan = await prepare(api, domainTarget, revision);
	await apply(api, plan, plan.digest);
	assert.equal(state.writes.length, 1);
	assert.equal(state.writes[0].method, "PUT");
	assert.match(state.writes[0].path, /\/workers\/domains$/u);
	assert.equal(state.domains[0].service, "example");
});
test("routes preserve unrelated paths and only create the two exact docs patterns", async () => {
	const other = {
		id: "unrelated",
		pattern: "example.test/docs-other",
		script: "api",
		request_limit_fail_open: false,
	};
	const { api, state } = fake({ routes: [other] });
	const plan = await prepare(api, routeTarget, revision);
	await apply(api, plan, plan.digest);
	assert.deepEqual(
		state.writes.map(({ body }) => body.pattern),
		routeTarget.patterns,
	);
	assert.deepEqual(
		state.routes.find(({ id }) => id === "unrelated"),
		other,
	);
});
test("conflicting route ownership is refused", async () => {
	const { api, state } = fake({
		routes: [
			{
				id: "conflict",
				pattern: "example.test/docs",
				script: "other",
				request_limit_fail_open: false,
			},
		],
	});
	await assert.rejects(prepare(api, routeTarget, revision), /conflicting/u);
	assert.deepEqual(state.writes, []);
});
test("403 fails closed without exposing the token or API response", async () => {
	const token = "synthetic-test-secret";
	const api = createApi(token, async () => ({
		ok: false,
		status: 403,
		json: () => {
			throw new Error(token);
		},
	}));
	await assert.rejects(
		api("GET", "/test"),
		(error) => error.message.includes("403") && !error.message.includes(token),
	);
});
test("incomplete domain pagination prevents a misleading empty plan", async () => {
	const { api } = fake();
	const incomplete = (method, path, body) =>
		path.includes("/workers/domains?") ? { result: [] } : api(method, path, body);
	await assert.rejects(prepare(incomplete, domainTarget, revision), /pagination/u);
});

const hostileSources = [
	'import { defineConfig } from "cf/config"; // worker: { name: "example", domains: ["example.test"] }\nexport default defineConfig({worker:{name:actualName,domains:actualDomains}});',
	'import { defineConfig } from "cf/config"; export default defineConfig({worker:{name:"example",domains:["example.test"],...override}});',
	'import { defineConfig } from "cf/config"; if(false){const config={worker:{name:"example",domains:["example.test"]}}} export default defineConfig(dynamic);',
	'import { defineConfig } from "cf/config"; export default defineConfig({worker:{name:"example",domains:["example.test"],domains:dynamic}});',

	'import { defineConfig } from "cf/config"; const isPreview=true; export default defineConfig({worker:{name:"example",domains:isPreview?[]:["example.test"]}});',
	'const defineConfig=()=>({worker:{name:"other",domains:["other.test"]}}); export default defineConfig({worker:{name:"example",domains:["example.test"]}});',
	'import { defineConfig } from "cf/config"; export default defineConfig({accountId:"other",worker:{name:"example",domains:["example.test"]}});',
];
for (const [index, source] of hostileSources.entries()) {
	test(`source parser rejects hostile configuration ${index}`, async () => {
		const root = await mkdtemp(`${tmpdir()}/hostname-hostile-`);
		try {
			await writeFile(`${root}/config.ts`, source);
			await assert.rejects(validateSource({ ...domainTarget, config: "config.ts" }, root));
		} finally {
			await rm(root, { recursive: true, force: true });
		}
	});
}
test("overlapping domain pages are refused", async () => {
	const { api } = fake();
	const overlap = (method, path, body) =>
		path.includes("/workers/domains?")
			? {
					result: [attachment],
					result_info: {
						page: Number(new URL(`https://example.test${path}`).searchParams.get("page")),
						per_page: 100,
						total_count: 2,
					},
				}
			: api(method, path, body);
	await assert.rejects(prepare(overlap, domainTarget, revision), /repeated/u);
});

test("route source rejects a local triggers factory", async () => {
	const root = await mkdtemp(`${tmpdir()}/hostname-route-source-`);
	try {
		await writeFile(
			`${root}/config.ts`,
			'import { defineConfig } from "cf/config"; const triggers={fetch:()=>({pattern:"other.test/*"})}; export default defineConfig({worker:{name:"docs",triggers:[triggers.fetch({pattern:"example.test/docs",zone:"test"}),triggers.fetch({pattern:"example.test/docs/*",zone:"test"})]}});',
		);
		await assert.rejects(
			validateSource({ ...routeTarget, config: "config.ts", zone: "test" }, root),
		);
	} finally {
		await rm(root, { recursive: true, force: true });
	}
});

test("route source rejects mutation of the imported factory", async () => {
	const root = await mkdtemp(`${tmpdir()}/hostname-route-mutation-`);
	try {
		await writeFile(
			`${root}/config.ts`,
			'import { defineConfig,triggers } from "cf/config"; triggers.fetch=()=>({pattern:"other.test/*"}); export default defineConfig({worker:{name:"docs",triggers:[triggers.fetch({pattern:"example.test/docs",zone:"test"}),triggers.fetch({pattern:"example.test/docs/*",zone:"test"})]}});',
		);
		await assert.rejects(
			validateSource({ ...routeTarget, config: "config.ts", zone: "test" }, root),
		);
	} finally {
		await rm(root, { recursive: true, force: true });
	}
});
