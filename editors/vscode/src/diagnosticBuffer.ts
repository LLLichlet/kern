type PendingDiagnostics<TUri, TDiagnostic> = {
    uri: TUri;
    diagnostics: readonly TDiagnostic[];
};

export class DiagnosticBuffer<TUri, TDiagnostic> {
    private readonly delayMs: number;
    private readonly pending = new Map<
        string,
        {
            timer: NodeJS.Timeout;
            payload: PendingDiagnostics<TUri, TDiagnostic>;
        }
    >();

    constructor(delayMs: number) {
        this.delayMs = delayMs;
    }

    schedule(
        key: string,
        payload: PendingDiagnostics<TUri, TDiagnostic>,
        flush: (payload: PendingDiagnostics<TUri, TDiagnostic>) => void,
    ): void {
        const existing = this.pending.get(key);
        if (existing) {
            clearTimeout(existing.timer);
        }

        const timer = setTimeout(() => {
            this.pending.delete(key);
            flush(payload);
        }, this.delayMs);

        this.pending.set(key, { timer, payload });
    }

    flushAll(flush: (payload: PendingDiagnostics<TUri, TDiagnostic>) => void): void {
        for (const [key, entry] of this.pending) {
            clearTimeout(entry.timer);
            flush(entry.payload);
            this.pending.delete(key);
        }
    }

    clear(): void {
        for (const entry of this.pending.values()) {
            clearTimeout(entry.timer);
        }
        this.pending.clear();
    }
}
