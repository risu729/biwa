import { createHash } from "node:crypto";
import { readFile, appendFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";

import sourceGuard from "./hostname-source.mjs";

const account = "59ea63cc00914b30ca410b062ae2bb7f";
export const targets = {
	worker: {
		kind: "domain",
		worker: "biwa-docs",
		config: "docs/cloudflare.config.ts",
		hostname: "biwa.takuk.me",
		zone: "takuk.me",
		zoneId: "e5f785a1c55a37a4b7c65ee832002237",
	},
};
const apiRoot = "https://api.cloudflare.com/client/v4";
const stable = (value) =>
	JSON.stringify(value, (_, item) =>
		item && typeof item === "object" && !Array.isArray(item)
			? Object.fromEntries(Object.entries(item).sort(([a], [b]) => a.localeCompare(b)))
			: item,
	);
const same = (a, b) => stable(a) === stable(b);
class MaintenanceError extends Error {}
const requireThat = (condition, message) => {
	if (!condition) throw new MaintenanceError(message);
};

export async function validateSource(target, root = ".") {
	const source = await readFile(`${root}/${target.config}`, "utf8");
	const actual = sourceGuard.extract(source, target.kind);
	const expected =
		target.kind === "domain"
			? [target.hostname]
			: target.patterns.map((pattern) => ({ pattern, zone: target.zone }));
	requireThat(
		(actual.accountId === null || actual.accountId === account) &&
			actual.name === target.worker &&
			same(actual.values, expected),
		"Production source differs from the reviewed hostname allowlist.",
	);
}

export function createApi(token, fetcher = fetch) {
	requireThat(token && !/[\r\n]/u.test(token), "A Cloudflare token is required.");
	return async (method, path, body) => {
		let response;
		try {
			response = await fetcher(`${apiRoot}${path}`, {
				method,
				redirect: "error",
				signal: AbortSignal.timeout(30000),
				headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
				...(body ? { body: JSON.stringify(body) } : {}),
			});
		} catch {
			throw new MaintenanceError(
				"Cloudflare transport failed; no automatic retry or permission change is attempted.",
			);
		}
		// Never emit response bodies, headers, credentials or transport error causes.
		requireThat(
			response.ok,
			`Cloudflare ${method} failed with HTTP ${response.status}; no permission is added automatically.`,
		);
		const envelope = await response.json();
		requireThat(
			envelope.success === true && envelope.result !== undefined,
			"Cloudflare returned an invalid result.",
		);
		return envelope;
	};
}

async function domains(api) {
	const records = [];
	const seen = new Set();
	let expectedTotal = null;
	for (let page = 1; page <= 100; page++) {
		const response = await api(
			"GET",
			`/accounts/${account}/workers/domains?page=${page}&per_page=100`,
		);
		requireThat(Array.isArray(response.result), "Domain inventory is not a list.");
		requireThat(
			response.result_info?.page === page && response.result_info?.per_page === 100,
			"Domain pagination returned an unexpected page.",
		);
		requireThat(
			expectedTotal === null || expectedTotal === response.result_info?.total_count,
			"Domain pagination total changed during reading.",
		);
		expectedTotal = response.result_info?.total_count;
		for (const entry of response.result) {
			requireThat(
				typeof entry.id === "string" && !seen.has(entry.id),
				"Domain pagination repeated an attachment.",
			);
			seen.add(entry.id);
		}
		records.push(...response.result);
		const total = response.result_info?.total_count;
		requireThat(
			Number.isInteger(total) && total >= records.length,
			"Domain pagination is incomplete.",
		);
		if (records.length === total) return records.sort((a, b) => a.id.localeCompare(b.id));
		requireThat(response.result.length > 0, "Domain pagination made no progress.");
	}
	throw new Error("Domain pagination exceeded its bound.");
}

async function snapshot(api, target) {
	const base = `/accounts/${account}/workers/scripts/${target.worker}`;
	const schedules = (await api("GET", `${base}/schedules`)).result;
	const subdomain = (await api("GET", `${base}/subdomain`)).result;
	if (target.kind === "domain") return { domains: await domains(api), schedules, subdomain };
	const routes = (await api("GET", `/zones/${target.zoneId}/workers/routes`)).result;
	requireThat(Array.isArray(routes), "Route inventory is not a list.");
	return { routes: routes.sort((a, b) => a.id.localeCompare(b.id)), schedules, subdomain };
}

export async function prepare(api, target, revision) {
	requireThat(/^[a-f0-9]{40}$/u.test(revision), "A full reviewed revision is required.");
	const before = await snapshot(api, target);
	const changes = [];
	if (target.kind === "domain") {
		const matches = before.domains.filter((domain) => domain.hostname === target.hostname);
		requireThat(matches.length <= 1, "Duplicate hostname attachments require review.");
		if (matches.length) {
			const domain = matches[0];
			requireThat(
				domain.service === target.worker &&
					domain.zone_id === target.zoneId &&
					domain.environment === "production" &&
					domain.enabled === true &&
					domain.previews_enabled === false,
				"An existing domain has conflicting ownership or settings; replacement is refused.",
			);
		} else {
			// An initial attachment may replace DNS implicitly; require a complete empty DNS result first.
			const dns = await api(
				"GET",
				`/zones/${target.zoneId}/dns_records?name=${target.hostname}&per_page=100`,
			);
			requireThat(
				Array.isArray(dns.result) && dns.result.length === 0 && dns.result_info?.total_count === 0,
				"Existing or incompletely listed DNS prevents domain attachment.",
			);
			changes.push({
				method: "PUT",
				path: `/accounts/${account}/workers/domains`,
				body: {
					hostname: target.hostname,
					service: target.worker,
					zone_id: target.zoneId,
					enabled: true,
					previews_enabled: false,
				},
			});
		}
	} else {
		for (const pattern of target.patterns) {
			const matches = before.routes.filter((route) => route.pattern === pattern);
			requireThat(matches.length <= 1, "Duplicate routes require review.");
			if (matches.length)
				requireThat(
					matches[0].script === target.worker && matches[0].request_limit_fail_open === false,
					"An existing route has conflicting ownership or settings; replacement is refused.",
				);
			else
				changes.push({
					method: "POST",
					path: `/zones/${target.zoneId}/workers/routes`,
					body: { pattern, script: target.worker, request_limit_fail_open: false },
				});
		}
	}
	const digest = createHash("sha256")
		.update(stable({ revision, target, before, changes }))
		.digest("hex");
	return { revision, target, before, changes, digest };
}

export async function apply(api, plan, expectedDigest) {
	requireThat(
		/^[a-f0-9]{64}$/u.test(expectedDigest) && expectedDigest === plan.digest,
		"The current plan differs from the explicitly reviewed digest. Run plan again.",
	);
	// Re-read immediately before mutation; unrelated state changes invalidate the plan too.
	const current = await prepare(api, plan.target, plan.revision);
	requireThat(
		current.digest === plan.digest,
		"Cloudflare changed after planning; no write was attempted.",
	);
	for (const change of plan.changes) {
		// oxlint-disable-next-line eslint/no-await-in-loop -- Stop sequential mutations on the first ambiguous API failure.
		await api(change.method, change.path, change.body);
	}
	const after = await prepare(api, plan.target, plan.revision);
	requireThat(after.changes.length === 0, "Readback did not confirm every intended attachment.");
	requireThat(
		same(plan.before.schedules, after.before.schedules) &&
			same(plan.before.subdomain, after.before.subdomain),
		"Unrelated schedules or workers.dev settings changed during maintenance.",
	);
	const key = plan.target.kind === "domain" ? "domains" : "routes";
	const intended = (entry) =>
		plan.target.kind === "domain"
			? entry.hostname === plan.target.hostname
			: plan.target.patterns.includes(entry.pattern);
	requireThat(
		same(
			plan.before[key].filter((entry) => !intended(entry)),
			after.before[key].filter((entry) => !intended(entry)),
		),
		"An unrelated hostname or route changed during maintenance.",
	);
	return after;
}

async function main() {
	const operation = process.env.MAINTENANCE_OPERATION ?? "plan";
	const target = targets[process.env.MAINTENANCE_TARGET ?? "worker"];
	requireThat(operation === "plan" || operation === "apply", "Choose plan or apply.");
	requireThat(target, "Unknown maintenance target.");
	requireThat(process.env.GITHUB_REF === "refs/heads/main", "Maintenance is restricted to main.");
	requireThat(process.env.CLOUDFLARE_ACCOUNT_ID === account, "Unexpected Cloudflare account.");
	await validateSource(target);
	const api = createApi(process.env.CLOUDFLARE_API_TOKEN);
	const plan = await prepare(api, target, process.env.GITHUB_SHA);
	const summary = `Hostname maintenance: ${operation}\nRevision: ${plan.revision}\nPlan SHA-256: ${plan.digest}\nChanges: ${JSON.stringify(plan.changes)}\n`;
	console.log(summary);
	if (process.env.GITHUB_STEP_SUMMARY)
		await appendFile(process.env.GITHUB_STEP_SUMMARY, `\n\`\`\`text\n${summary}\`\`\`\n`);
	if (operation === "apply") {
		await apply(api, plan, process.env.EXPECTED_PLAN_SHA256 ?? "");
		console.log(
			"API readback verified the requested attachments and unchanged schedules, workers.dev and unrelated routing.",
		);
	}
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
	main().catch((error) => {
		console.error(
			error instanceof MaintenanceError
				? error.message
				: "Hostname maintenance failed; inspect source and API metadata before retrying.",
		);
		process.exitCode = 1;
	});
}
