#!/usr/bin/env bun
//MISE description="Deploy the VitePress docs with Cloudflare Wrangler."

import { createHash } from "node:crypto";
import { appendFile, mkdir, readdir, rm } from "node:fs/promises";
import { join, resolve } from "node:path";

import { $ } from "bun";

type Mode = "preview" | "production";

type WranglerOutputEntry =
	| {
			type: "deploy";
			targets?: string[];
	  }
	| {
			type: "version-upload";
			preview_url?: string;
			preview_alias_url?: string;
	  }
	| {
			type: string;
	  };

const repoRoot = resolve(import.meta.dirname, "../..");
const docsDir = join(repoRoot, "docs");
const configPath = ".vitepress/dist/wrangler.json";
const workerName = process.env["WORKER_NAME"] ?? "biwa-docs";

await main();

async function main() {
	const { mode, wranglerArgs } = parseArgs(process.argv.slice(2));
	const dryRun = wranglerArgs.includes("--dry-run");
	const outputDir = join(
		process.env["RUNNER_TEMP"] ?? "/tmp",
		`wrangler-output-${mode}-${process.pid}`,
	);

	process.chdir(docsDir);
	process.env["WRANGLER_OUTPUT_FILE_DIRECTORY"] = outputDir;

	await rm(outputDir, { force: true, recursive: true });
	await mkdir(outputDir, { recursive: true });

	if (mode === "preview") {
		await uploadPreview({ dryRun, outputDir, wranglerArgs });
		return;
	}

	await deployProduction({ outputDir, wranglerArgs });
}

function parseArgs(args: string[]): { mode: Mode; wranglerArgs: string[] } {
	if (args[0] === "preview" || args[0] === "production") {
		return { mode: args[0], wranglerArgs: args.slice(1) };
	}
	if (args[0] && !args[0].startsWith("-")) {
		throw new Error("Usage: mise run docs:deploy -- [preview|production] [wrangler args...]");
	}
	return { mode: "production", wranglerArgs: args };
}

async function uploadPreview({
	dryRun,
	outputDir,
	wranglerArgs,
}: {
	dryRun: boolean;
	outputDir: string;
	wranglerArgs: string[];
}) {
	const branchName = await resolveBranchName();
	const previewAlias =
		process.env["PREVIEW_ALIAS"] ?? resolvePreviewAlias({ branchName, workerName });

	await $`wrangler versions upload --config ${configPath} --preview-alias ${previewAlias} ${wranglerArgs}`;

	const upload = await findWranglerOutput({ outputDir, type: "version-upload" });
	if (!upload) {
		if (dryRun) {
			return;
		}
		throw new Error("Unable to find Wrangler version upload output");
	}

	const previewUrl = upload.preview_url ?? "";
	const previewAliasUrl = upload.preview_alias_url ?? "";
	await appendOutput([`preview-url=${previewUrl}`, `preview-alias-url=${previewAliasUrl}`, ""]);
	await appendSummary([
		"### Cloudflare Workers Preview",
		"",
		`Branch alias: \`${previewAlias}\``,
		"",
		"| Name | URL |",
		"| - | - |",
		`| Version preview | ${link(previewUrl)} |`,
		`| Branch alias preview | ${link(previewAliasUrl)} |`,
		"",
	]);
}

async function deployProduction({
	outputDir,
	wranglerArgs,
}: {
	outputDir: string;
	wranglerArgs: string[];
}) {
	await $`wrangler deploy --config ${configPath} ${wranglerArgs}`;

	const deploy = await findWranglerOutput({ outputDir, type: "deploy" });
	const targets = deploy?.targets ?? [];
	await appendSummary([
		"### Cloudflare Workers Production Deploy",
		"",
		"| Target |",
		"| - |",
		...(targets.length > 0 ? targets.map((target) => `| <${target}> |`) : ["| _Unavailable_ |"]),
		"",
	]);
}

async function resolveBranchName(): Promise<string> {
	const branchName = process.env["BRANCH_NAME"];
	if (branchName) {
		return branchName;
	}
	const headRef = process.env["GITHUB_HEAD_REF"];
	if (headRef) {
		return headRef;
	}
	const refName = process.env["GITHUB_REF_NAME"];
	if (refName) {
		return refName;
	}
	try {
		return (await $`git rev-parse --abbrev-ref HEAD`.quiet().text()).trim();
	} catch {
		return "";
	}
}

function resolvePreviewAlias({
	branchName,
	workerName,
}: {
	branchName: string;
	workerName: string;
}): string {
	const fallbackSha = process.env["GITHUB_SHA"]?.slice(0, 8) ?? "local";
	let alias = branchName
		.toLowerCase()
		.replaceAll(/[^a-z0-9-]+/g, "-")
		.replaceAll(/-+/g, "-")
		.replaceAll(/^-+|-+$/g, "");

	if (!alias) {
		alias = `branch-${fallbackSha}`;
	}
	if (!/^[a-z]/.test(alias)) {
		alias = `branch-${alias}`;
	}

	const maxLength = 63 - workerName.length - 1;
	if (maxLength < 1) {
		throw new Error("Worker name is too long for a preview alias");
	}
	if (alias.length <= maxLength) {
		return alias;
	}

	const hash = createHash("sha256").update(branchName).digest("hex").slice(0, 4);
	const prefixLength = maxLength - hash.length - 1;
	if (prefixLength < 1) {
		throw new Error("Worker name leaves no room for a preview alias hash");
	}
	return `${alias.slice(0, prefixLength)}-${hash}`;
}

async function findWranglerOutput<Type extends WranglerOutputEntry["type"]>({
	outputDir,
	type,
}: {
	outputDir: string;
	type: Type;
}): Promise<Extract<WranglerOutputEntry, { type: Type }> | undefined> {
	for (const path of await readdir(outputDir)) {
		if (!path.startsWith("wrangler-output-") || !path.endsWith(".json")) {
			continue;
		}
		for await (const line of readLines(join(outputDir, path))) {
			const entry = JSON.parse(line) as WranglerOutputEntry;
			if (entry.type === type) {
				return entry as Extract<WranglerOutputEntry, { type: Type }>;
			}
		}
	}
	return undefined;
}

async function* readLines(path: string): AsyncGenerator<string> {
	const reader = Bun.file(path).stream().pipeThrough(new TextDecoderStream()).getReader();
	let buffered = "";

	while (true) {
		const { value, done } = await reader.read();
		if (done) {
			break;
		}

		buffered += value;
		const lines = buffered.split("\n");
		buffered = lines.pop() ?? "";
		for (const line of lines) {
			const trimmed = line.trim();
			if (trimmed) {
				yield trimmed;
			}
		}
	}

	const trimmed = buffered.trim();
	if (trimmed) {
		yield trimmed;
	}
}

async function appendOutput(lines: string[]) {
	await appendGitHubFile("GITHUB_OUTPUT", lines);
}

async function appendSummary(lines: string[]) {
	await appendGitHubFile("GITHUB_STEP_SUMMARY", lines);
}

async function appendGitHubFile(name: string, lines: string[]) {
	const path = process.env[name];
	if (!path) {
		return;
	}
	await appendFile(path, lines.join("\n"), "utf8");
}

function link(url: string) {
	return url ? `<${url}>` : "_Unavailable_";
}
