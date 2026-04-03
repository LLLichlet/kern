import test from "node:test";
import assert from "node:assert/strict";
import {
    createAutoSuggestRequest,
    isCompletionResultEmpty,
    matchesAutoSuggestRequest,
    shouldAutoTriggerSuggest,
    shouldHideAutoTriggeredSuggest,
} from "../clientBehavior";

test("auto-trigger suggest fires for identifier and member characters", () => {
    assert.equal(shouldAutoTriggerSuggest("l", 0, "l", 1), true);
    assert.equal(shouldAutoTriggerSuggest("e", 0, "le", 2), true);
    assert.equal(shouldAutoTriggerSuggest("x", 0, "xx", 2), true);
});

test("auto-trigger suggest ignores replacements and multiline edits", () => {
    assert.equal(shouldAutoTriggerSuggest("ab", 0, "ab", 2), false);
    assert.equal(shouldAutoTriggerSuggest("\n", 0, "\n", 1), false);
    assert.equal(shouldAutoTriggerSuggest("x", 1, "x", 1), false);
});

test("auto-trigger suggest avoids first-character noise outside keyword prefixes", () => {
    assert.equal(shouldAutoTriggerSuggest("x", 0, "x", 1), false);
    assert.equal(shouldAutoTriggerSuggest("p", 0, "value.p", 7), false);
});

test("auto-trigger request matching expires and respects document position", () => {
    const request = createAutoSuggestRequest("file:///main.rn", 3, 12, 100);

    assert.equal(
        matchesAutoSuggestRequest(request, "file:///main.rn", 3, 12, 200),
        true,
    );
    assert.equal(
        matchesAutoSuggestRequest(request, "file:///main.rn", 4, 12, 200),
        false,
    );
    assert.equal(
        matchesAutoSuggestRequest(request, "file:///main.rn", 3, 12, 2000),
        false,
    );
});

test("empty completion results are detected for arrays and lists", () => {
    assert.equal(isCompletionResultEmpty(undefined), true);
    assert.equal(isCompletionResultEmpty([]), true);
    assert.equal(isCompletionResultEmpty([{ label: "let" }]), false);
    assert.equal(isCompletionResultEmpty({ items: [] }), true);
    assert.equal(isCompletionResultEmpty({ items: [{ label: "let" }] }), false);
});

test("empty auto-triggered completion results are hidden", () => {
    assert.equal(shouldHideAutoTriggeredSuggest([], 0, undefined, true), true);
    assert.equal(shouldHideAutoTriggeredSuggest([], 1, ".", false), true);
    assert.equal(shouldHideAutoTriggeredSuggest([], 2, undefined, false), true);
    assert.equal(
        shouldHideAutoTriggeredSuggest([{ label: "let" }], 0, undefined, true),
        false,
    );
    assert.equal(shouldHideAutoTriggeredSuggest([], 0, undefined, false), false);
});
