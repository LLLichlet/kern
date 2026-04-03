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

export type AutoSuggestRequest = {
    documentUri: string;
    documentVersion: number;
    offset: number;
    requestedAt: number;
};

export function shouldAutoTriggerSuggest(
    text: string,
    rangeLength: number,
    documentText: string,
    insertedOffset: number,
): boolean {
    if (rangeLength !== 0) {
        return false;
    }
    if (text.length !== 1) {
        return false;
    }
    if (!/[A-Za-z0-9_]/.test(text)) {
        return false;
    }

    const prefixLength = identifierPrefixLengthAt(documentText, insertedOffset);
    if (prefixLength == 0) {
        return false;
    }

    const prefixStart = insertedOffset - prefixLength;
    if (prefixStart > 0) {
        const prev = documentText[prefixStart - 1];
        if (prev === ".") {
            return false;
        }
    }

    if (prefixLength === 1) {
        return FIRST_CHAR_KEYWORD_PREFIXES.has(text);
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

    return triggerKind === 1 || triggerKind === 2 || triggerCharacter === ".";
}

function identifierPrefixLengthAt(documentText: string, insertedOffset: number): number {
    let start = insertedOffset;
    while (start > 0 && /[A-Za-z0-9_]/.test(documentText[start - 1])) {
        start -= 1;
    }
    return insertedOffset - start;
}
