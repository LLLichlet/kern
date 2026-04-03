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

function identifierPrefixLengthAt(documentText: string, insertedOffset: number): number {
    let start = insertedOffset;
    while (start > 0 && /[A-Za-z0-9_]/.test(documentText[start - 1])) {
        start -= 1;
    }
    return insertedOffset - start;
}
