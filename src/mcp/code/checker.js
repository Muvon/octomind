// Copyright 2026 Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// The check every `code` script passes before it runs. Loaded after the
// TypeScript compiler (`ts`) and the standard library typings (`__libs`).
//
// `__check(declarations, script)` type-checks the script against the tool
// declarations and returns JSON: either `{problems: [{line, column, message}]}`
// or `{javascript, lines}`, where `javascript` is the script ready to run and
// `lines[n - 1]` is the script line that line `n` of `javascript` came from.
globalThis.__check = (() => {
	// The script runs as the body of an async function, so `await` and `return`
	// work at its top level. Nothing is put before the first line break, so
	// line numbers stay the script's own.
	const PREFIX = "(async () => {";
	const SUFFIX = "\n})().then(__ok, __fail);";
	const RETURN = "return ";
	const LIB_DIR = "/lib/";
	const DEFAULT_LIB = "lib.es2023.d.ts";
	const DECLARATIONS = "/declarations.d.ts";
	const SCRIPT = "/script.ts";

	const files = new Map();
	const set = (name, text) => {
		const known = files.get(name);
		if (!known || known.text !== text) files.set(name, { text, version: (known ? known.version : 0) + 1 });
	};
	const read = (name) => {
		if (files.has(name)) return files.get(name).text;
		return name.startsWith(LIB_DIR) ? __libs[name.slice(LIB_DIR.length)] : undefined;
	};

	const options = {
		target: ts.ScriptTarget.ES2022,
		lib: [DEFAULT_LIB],
		strict: true,
		// `catch (error) { error.message }` is how scripts read a failed tool.
		useUnknownInCatchVariables: false,
		skipLibCheck: true,
		types: [],
		sourceMap: true,
		newLine: ts.NewLineKind.LineFeed,
	};
	const host = {
		getCompilationSettings: () => options,
		getScriptFileNames: () => [DECLARATIONS, SCRIPT],
		getScriptVersion: (name) => String(files.has(name) ? files.get(name).version : 0),
		getScriptSnapshot: (name) => {
			const text = read(name);
			return text === undefined ? undefined : ts.ScriptSnapshot.fromString(text);
		},
		getCurrentDirectory: () => "/",
		getDefaultLibFileName: () => LIB_DIR + DEFAULT_LIB,
		fileExists: (name) => read(name) !== undefined,
		readFile: read,
		directoryExists: () => true,
		getDirectories: () => [],
		useCaseSensitiveFileNames: () => true,
	};
	// One service for the life of the process: the library typings are parsed
	// once, and each later check only re-reads the two files that changed.
	const service = ts.createLanguageService(host, ts.createDocumentRegistry());

	const BASE64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
	// For each generated line, the 1-based source line of its first mapping
	// (0 when the line has none). `mappings` is the source map's VLQ text.
	const sourceLines = (mappings) => {
		const lines = [];
		let sourceLine = 0;
		for (const group of mappings.split(";")) {
			let first = 0;
			for (const segment of group.split(",")) {
				const fields = [];
				let value = 0;
				let shift = 0;
				for (const character of segment) {
					const digit = BASE64.indexOf(character);
					value += (digit & 31) << shift;
					if (digit & 32) {
						shift += 5;
					} else {
						fields.push(value & 1 ? -(value >> 1) : value >> 1);
						value = 0;
						shift = 0;
					}
				}
				if (fields.length >= 4) {
					sourceLine += fields[2];
					if (first === 0) first = sourceLine + 1;
				}
			}
			lines.push(first);
		}
		return lines;
	};

	const exported = (statement) =>
		ts.canHaveModifiers(statement) &&
		(ts.getModifiers(statement) || []).some((modifier) => modifier.kind === ts.SyntaxKind.ExportKeyword);

	return (declarations, script) => {
		const plain = ts.createSourceFile("/plain.ts", script, options.target, false, ts.ScriptKind.TS);
		const place = (offset) => {
			const at = plain.getLineAndCharacterOfPosition(Math.max(0, Math.min(offset, script.length)));
			return { line: at.line + 1, column: at.character + 1 };
		};
		const problems = [];
		for (const statement of plain.statements) {
			if (
				ts.isImportDeclaration(statement) ||
				ts.isImportEqualsDeclaration(statement) ||
				ts.isExportDeclaration(statement) ||
				ts.isExportAssignment(statement) ||
				exported(statement)
			) {
				problems.push({
					...place(statement.getStart(plain)),
					message: "import and export are not available: a script can only call the listed tools",
				});
			}
		}

		// The compiler's own words for these ("can only be used at the top level
		// of a module") would send the model looking for a module to write.
		if (problems.length) return JSON.stringify({ problems });

		// A trailing expression is the script's result: `return` goes in front of it.
		const last = plain.statements[plain.statements.length - 1];
		const split = last && ts.isExpressionStatement(last) ? last.getStart(plain) : -1;
		const body = split < 0 ? script : script.slice(0, split) + RETURN + script.slice(split);
		// An offset in the wrapped text, as an offset in the script as written.
		const written = (offset) => {
			const at = offset - PREFIX.length;
			if (split < 0 || at <= split) return at;
			return Math.max(split, at - RETURN.length);
		};

		set(DECLARATIONS, declarations);
		set(SCRIPT, PREFIX + body + SUFFIX);
		// Declarations that do not parse would type every tool as `any` and let
		// any script through, so a broken catalog stops the check instead.
		const broken = service.getSyntacticDiagnostics(DECLARATIONS)[0];
		if (broken) {
			const reason = ts.flattenDiagnosticMessageText(broken.messageText, "\n");
			const message = `the tool declarations are invalid (not a problem in the script): ${reason}`;
			return JSON.stringify({ problems: [{ line: 0, column: 0, message }] });
		}
		const found = [...service.getSyntacticDiagnostics(SCRIPT), ...service.getSemanticDiagnostics(SCRIPT)];
		for (const diagnostic of found) {
			problems.push({
				...place(written(diagnostic.start || 0)),
				message: `TS${diagnostic.code}: ${ts.flattenDiagnosticMessageText(diagnostic.messageText, "\n")}`,
			});
		}
		if (problems.length) return JSON.stringify({ problems });

		const output = service.getEmitOutput(SCRIPT).outputFiles;
		const javascript = output.find((file) => file.name.endsWith(".js")).text;
		const map = JSON.parse(output.find((file) => file.name.endsWith(".map")).text);
		// The wrapper's closing line is not a line of the script.
		const scriptLines = plain.getLineStarts().length;
		const lines = sourceLines(map.mappings).map((line) => (line > scriptLines ? 0 : line));
		return JSON.stringify({ javascript, lines });
	};
})();
