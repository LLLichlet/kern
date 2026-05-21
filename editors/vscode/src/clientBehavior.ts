const FIRST_CHAR_KEYWORD_PREFIXES = new Set([
    "a",
    "b",
    "c",
    "d",
    "e",
    "f",
    "i",
    "l",
    "m",
    "o",
    "p",
    "r",
    "s",
    "t",
    "u",
    "v",
]);

const AUTO_SUGGEST_REQUEST_TTL_MS = 1500;

export type AutoSuggestMode = "off" | "keywords" | "aggressive";
type LexState = "code" | "blockComment" | "doubleQuote" | "singleQuote";

export type TextPosition = {
    line: number;
    character: number;
};

export type AutoSuggestDocument = {
    lines: string[];
    lineStates: LexState[];
};

export type AutoSuggestRequest = {
    documentUri: string;
    documentVersion: number;
    offset: number;
    requestedAt: number;
};

export function createAutoSuggestDocument(text: string): AutoSuggestDocument {
    const lines = splitDocumentLines(text);
    const lineStates = computeLineStates(lines);
    return { lines, lineStates };
}

export function updateAutoSuggestDocument(
    document: AutoSuggestDocument,
    rangeStart: TextPosition,
    rangeEnd: TextPosition,
    text: string,
): AutoSuggestDocument {
    const nextLines = document.lines.slice();
    const startLine = clampLineIndex(nextLines, rangeStart.line);
    const endLine = clampLineIndex(nextLines, rangeEnd.line);
    const startCharacter = clampCharacter(nextLines[startLine], rangeStart.character);
    const endCharacter = clampCharacter(nextLines[endLine], rangeEnd.character);

    const prefix = nextLines[startLine].slice(0, startCharacter);
    const suffix = nextLines[endLine].slice(endCharacter);
    const insertedLines = splitDocumentLines(text);
    const replacementLines =
        insertedLines.length === 1
            ? [`${prefix}${insertedLines[0]}${suffix}`]
            : [
                  `${prefix}${insertedLines[0]}`,
                  ...insertedLines.slice(1, -1),
                  `${insertedLines[insertedLines.length - 1]}${suffix}`,
              ];
    nextLines.splice(startLine, endLine - startLine + 1, ...replacementLines);

    const lineStates = document.lineStates.slice(0, startLine);
    let state =
        startLine > 0
            ? scanLine(nextLines[startLine - 1], lineStates[startLine - 1])
            : "code";

    for (let line = startLine; line < nextLines.length; line += 1) {
        lineStates[line] = state;
        state = scanLine(nextLines[line], state);
    }

    return {
        lines: nextLines,
        lineStates,
    };
}

export function shouldAutoTriggerSuggest(
    mode: AutoSuggestMode,
    text: string,
    rangeLength: number,
    document: AutoSuggestDocument,
    position: TextPosition,
): boolean {
    if (mode === "off") {
        return false;
    }
    if (rangeLength !== 0) {
        return false;
    }
    if (text.length !== 1) {
        return false;
    }
    if (!/[A-Za-z0-9_]/.test(text)) {
        return false;
    }
    if (!isCodePosition(document, position)) {
        return false;
    }

    const prefixLength = identifierPrefixLengthAt(document, position);
    if (prefixLength == 0) {
        return false;
    }

    const line = document.lines[position.line] ?? "";
    const prefixStart = position.character - prefixLength;
    if (prefixStart > 0 && line[prefixStart - 1] === ".") {
        return false;
    }

    if (mode === "keywords") {
        return (
            prefixLength === 1 &&
            FIRST_CHAR_KEYWORD_PREFIXES.has(text.toLowerCase())
        );
    }

    if (prefixLength === 1) {
        return FIRST_CHAR_KEYWORD_PREFIXES.has(text.toLowerCase());
    }

    return true;
}

export function createAutoSuggestRequest(
    documentUri: string,
    documentVersion: number,
    offset: number,
    requestedAt: number,
): AutoSuggestRequest {
    return {
        documentUri,
        documentVersion,
        offset,
        requestedAt,
    };
}

export function matchesAutoSuggestRequest(
    request: AutoSuggestRequest | undefined,
    documentUri: string,
    documentVersion: number,
    offset: number,
    now: number,
): boolean {
    if (!request) {
        return false;
    }

    if (now - request.requestedAt > AUTO_SUGGEST_REQUEST_TTL_MS) {
        return false;
    }

    return (
        request.documentUri === documentUri &&
        request.documentVersion === documentVersion &&
        request.offset === offset
    );
}

export function isCompletionResultEmpty(
    result: readonly unknown[] | { items: readonly unknown[] } | null | undefined,
): boolean {
    if (!result) {
        return true;
    }

    if (Array.isArray(result)) {
        return result.length === 0;
    }

    if (!("items" in result)) {
        return true;
    }

    return result.items.length === 0;
}

export function shouldHideAutoTriggeredSuggest(
    result: readonly unknown[] | { items: readonly unknown[] } | null | undefined,
    triggerKind: number,
    triggerCharacter: string | undefined,
    autoSuggestMatched: boolean,
): boolean {
    if (!isCompletionResultEmpty(result)) {
        return false;
    }

    if (autoSuggestMatched) {
        return true;
    }

    return triggerKind === 2 || triggerKind === 3 || triggerCharacter === ".";
}

function identifierPrefixLengthAt(document: AutoSuggestDocument, position: TextPosition): number {
    const line = document.lines[position.line] ?? "";
    const end = clampCharacter(line, position.character);
    let start = end;
    while (start > 0 && /[A-Za-z0-9_]/.test(line[start - 1])) {
        start -= 1;
    }
    return end - start;
}

function isCodePosition(document: AutoSuggestDocument, position: TextPosition): boolean {
    const line = document.lines[position.line] ?? "";
    const limit = clampCharacter(line, position.character);
    let index = 0;
    let state: LexState = document.lineStates[position.line] ?? "code";

    while (index < limit) {
        const ch = line[index];
        const next = line[index + 1];

        switch (state) {
            case "code":
                if (ch === "/" && next === "/") {
                    return false;
                }
                if (ch === "\\" && next === "\\") {
                    return false;
                }
                if (ch === "/" && next === "*") {
                    state = "blockComment";
                    index += 2;
                    continue;
                }
                if (ch === "\"") {
                    state = "doubleQuote";
                    index += 1;
                    continue;
                }
                if (ch === "'") {
                    state = "singleQuote";
                    index += 1;
                    continue;
                }
                index += 1;
                continue;
            case "blockComment":
                if (ch === "*" && next === "/") {
                    state = "code";
                    index += 2;
                    continue;
                }
                index += 1;
                continue;
            case "doubleQuote":
                if (ch === "\\") {
                    index += Math.min(2, limit - index);
                    continue;
                }
                if (ch === "\"") {
                    state = "code";
                }
                index += 1;
                continue;
            case "singleQuote":
                if (ch === "\\") {
                    index += Math.min(2, limit - index);
                    continue;
                }
                if (ch === "'") {
                    state = "code";
                }
                index += 1;
                continue;
        }
    }

    return state === "code";
}

function computeLineStates(lines: string[]): LexState[] {
    const states: LexState[] = [];
    let state: LexState = "code";
    for (const line of lines) {
        states.push(state);
        state = scanLine(line, state);
    }
    return states;
}

function scanLine(line: string, initialState: LexState): LexState {
    let index = 0;
    let state = initialState;

    while (index < line.length) {
        const ch = line[index];
        const next = line[index + 1];

        switch (state) {
            case "code":
                if (ch === "/" && next === "/") {
                    return "code";
                }
                if (ch === "\\" && next === "\\") {
                    return "code";
                }
                if (ch === "/" && next === "*") {
                    state = "blockComment";
                    index += 2;
                    continue;
                }
                if (ch === "\"") {
                    state = "doubleQuote";
                    index += 1;
                    continue;
                }
                if (ch === "'") {
                    state = "singleQuote";
                    index += 1;
                    continue;
                }
                index += 1;
                continue;
            case "blockComment":
                if (ch === "*" && next === "/") {
                    state = "code";
                    index += 2;
                    continue;
                }
                index += 1;
                continue;
            case "doubleQuote":
                if (ch === "\\") {
                    index += Math.min(2, line.length - index);
                    continue;
                }
                if (ch === "\"") {
                    state = "code";
                }
                index += 1;
                continue;
            case "singleQuote":
                if (ch === "\\") {
                    index += Math.min(2, line.length - index);
                    continue;
                }
                if (ch === "'") {
                    state = "code";
                }
                index += 1;
                continue;
        }
    }

    return state;
}

function splitDocumentLines(text: string): string[] {
    return text.split(/\r\n|\n|\r/);
}

function clampLineIndex(lines: string[], line: number): number {
    if (lines.length === 0) {
        return 0;
    }
    return Math.max(0, Math.min(line, lines.length - 1));
}

function clampCharacter(line: string, character: number): number {
    return Math.max(0, Math.min(character, line.length));
}
