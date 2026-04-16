import test from "node:test";
import assert from "node:assert/strict";
import {
    createAutoSuggestDocument,
    createAutoSuggestRequest,
    isCompletionResultEmpty,
    matchesAutoSuggestRequest,
    shouldAutoTriggerSuggest,
    shouldHideAutoTriggeredSuggest,
    updateAutoSuggestDocument,
} from "../clientBehavior";

test("auto-trigger suggest fires for identifier and member characters", () => {
    assert.equal(
        shouldAutoTriggerSuggest(
            "keywords",
            "l",
            0,
            createAutoSuggestDocument("l"),
            { line: 0, character: 1 },
        ),
        true,
    );
    assert.equal(
        shouldAutoTriggerSuggest(
            "keywords",
            "e",
            0,
            createAutoSuggestDocument("le"),
            { line: 0, character: 2 },
        ),
        false,
    );
    assert.equal(
        shouldAutoTriggerSuggest(
            "aggressive",
            "x",
            0,
            createAutoSuggestDocument("xx"),
            { line: 0, character: 2 },
        ),
        true,
    );
});

test("auto-trigger suggest ignores replacements and multiline edits", () => {
    assert.equal(
        shouldAutoTriggerSuggest(
            "aggressive",
            "ab",
            0,
            createAutoSuggestDocument("ab"),
            { line: 0, character: 2 },
        ),
        false,
    );
    assert.equal(
        shouldAutoTriggerSuggest(
            "aggressive",
            "\n",
            0,
            createAutoSuggestDocument("\n"),
            { line: 1, character: 0 },
        ),
        false,
    );
    assert.equal(
        shouldAutoTriggerSuggest(
            "aggressive",
            "x",
            1,
            createAutoSuggestDocument("x"),
            { line: 0, character: 1 },
        ),
        false,
    );
});

test("auto-trigger suggest avoids first-character noise outside keyword prefixes", () => {
    assert.equal(
        shouldAutoTriggerSuggest(
            "keywords",
            "x",
            0,
            createAutoSuggestDocument("x"),
            { line: 0, character: 1 },
        ),
        false,
    );
    assert.equal(
        shouldAutoTriggerSuggest(
            "aggressive",
            "p",
            0,
            createAutoSuggestDocument("value.p"),
            { line: 0, character: 7 },
        ),
        false,
    );
});

test("auto-trigger suggest can be disabled", () => {
    assert.equal(
        shouldAutoTriggerSuggest(
            "off",
            "l",
            0,
            createAutoSuggestDocument("l"),
            { line: 0, character: 1 },
        ),
        false,
    );
});

test("auto-trigger suggest avoids comments and strings", () => {
    assert.equal(
        shouldAutoTriggerSuggest(
            "aggressive",
            "l",
            0,
            createAutoSuggestDocument("// l"),
            { line: 0, character: 4 },
        ),
        false,
    );
    assert.equal(
        shouldAutoTriggerSuggest(
            "aggressive",
            "l",
            0,
            createAutoSuggestDocument("/* l"),
            { line: 0, character: 4 },
        ),
        false,
    );
    assert.equal(
        shouldAutoTriggerSuggest(
            "aggressive",
            "l",
            0,
            createAutoSuggestDocument("\"l"),
            { line: 0, character: 2 },
        ),
        false,
    );
    assert.equal(
        shouldAutoTriggerSuggest(
            "aggressive",
            "l",
            0,
            createAutoSuggestDocument("'l"),
            { line: 0, character: 2 },
        ),
        false,
    );
});

test("auto-trigger suggest tracks multiline lexical state incrementally", () => {
    let document = createAutoSuggestDocument("let\nv");
    document = updateAutoSuggestDocument(
        document,
        { line: 0, character: 0 },
        { line: 0, character: 0 },
        "/*",
    );
    assert.equal(
        shouldAutoTriggerSuggest("aggressive", "v", 0, document, {
            line: 1,
            character: 1,
        }),
        false,
    );

    document = updateAutoSuggestDocument(
        document,
        { line: 0, character: 5 },
        { line: 0, character: 5 },
        "*/",
    );
    assert.equal(
        shouldAutoTriggerSuggest("aggressive", "v", 0, document, {
            line: 1,
            character: 1,
        }),
        true,
    );
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
    assert.equal(shouldHideAutoTriggeredSuggest([], 1, undefined, false), false);
    assert.equal(shouldHideAutoTriggeredSuggest([], 3, undefined, false), true);
    assert.equal(
        shouldHideAutoTriggeredSuggest([{ label: "let" }], 0, undefined, true),
        false,
    );
    assert.equal(shouldHideAutoTriggeredSuggest([], 0, undefined, false), false);
});
