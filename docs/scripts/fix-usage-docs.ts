const generatedDocs = ["docs/src/cli/activate.md", "docs/src/cli/index.md"];
const generatedUsage = "biwa activate [--shell <SHELL>] <SUBCOMMAND>";
const clapUsage = "biwa activate [--shell <SHELL>] [COMMAND]";

for (const path of generatedDocs) {
	const source = await Bun.file(path).text();
	if (!source.includes(generatedUsage) && !source.includes(clapUsage)) {
		throw new Error(`Could not find activate usage in ${path}`);
	}
	await Bun.write(path, source.replaceAll(generatedUsage, clapUsage));
}
