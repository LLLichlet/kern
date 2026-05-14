import test from "node:test";
import assert from "node:assert/strict";
import { DiagnosticBuffer } from "../diagnosticBuffer";

test("diagnostic buffer only flushes the latest payload for a document", async () => {
    const buffer = new DiagnosticBuffer<string, string>(10);
    const flushed: Array<{ uri: string; diagnostics: readonly string[] }> = [];

    buffer.schedule(
        "file:///demo.kn",
        { uri: "file:///demo.kn", diagnostics: ["old"] },
        (payload) => {
            flushed.push(payload);
        },
    );
    buffer.schedule(
        "file:///demo.kn",
        { uri: "file:///demo.kn", diagnostics: ["new"] },
        (payload) => {
            flushed.push(payload);
        },
    );

    await new Promise((resolve) => setTimeout(resolve, 30));

    assert.deepEqual(flushed, [
        { uri: "file:///demo.kn", diagnostics: ["new"] },
    ]);
});

test("diagnostic buffer can flush pending diagnostics eagerly", () => {
    const buffer = new DiagnosticBuffer<string, string>(1000);
    const flushed: Array<{ uri: string; diagnostics: readonly string[] }> = [];

    buffer.schedule(
        "file:///demo.kn",
        { uri: "file:///demo.kn", diagnostics: ["ready"] },
        (payload) => {
            flushed.push(payload);
        },
    );

    buffer.flushAll((payload) => {
        flushed.push(payload);
    });

    assert.deepEqual(flushed, [
        { uri: "file:///demo.kn", diagnostics: ["ready"] },
    ]);
});
