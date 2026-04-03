import test from "node:test";
import assert from "node:assert/strict";
import { shouldAutoTriggerSuggest } from "../clientBehavior";

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
