export function shouldAutoTriggerSuggest(text: string, rangeLength: number): boolean {
    if (rangeLength !== 0) {
        return false;
    }
    if (text.length !== 1) {
        return false;
    }

    return /^[A-Za-z0-9_.:]$/.test(text);
}
