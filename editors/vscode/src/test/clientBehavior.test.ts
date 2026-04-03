import test from "node:test";
import assert from "node:assert/strict";
import { shouldAutoTriggerSuggest } from "../clientBehavior";

test("auto-trigger suggest fires for identifier and member characters", () => {
    assert.equal(shouldAutoTriggerSuggest("a", 0), true);
    assert.equal(shouldAutoTriggerSuggest("_", 0), true);
    assert.equal(shouldAutoTriggerSuggest(".", 0), true);
    assert.equal(shouldAutoTriggerSuggest(":", 0), true);
});

test("auto-trigger suggest ignores replacements and multiline edits", () => {
    assert.equal(shouldAutoTriggerSuggest("ab", 0), false);
    assert.equal(shouldAutoTriggerSuggest("\n", 0), false);
    assert.equal(shouldAutoTriggerSuggest("x", 1), false);
});
