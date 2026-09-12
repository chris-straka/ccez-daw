/**
 * Track M: background transcription jobs.
 *
 * The DAW never blocks on transcription: `submitTranscription` starts the
 * provider on a yielded task and returns a `TranscriptionJob` the caller
 * polls (`poll`), awaits (`wait`), or abandons (`cancel`). Progress is
 * best-effort — output correctness never depends on it.
 */
import type { TranscriptionPlan } from "./types.js";
import type { TranscriptionProvider, TranscriptionInput } from "./sidecar.js";

export type JobStatus = "pending" | "running" | "done" | "cancelled" | "failed";

export interface JobProgress {
  status: JobStatus;
  /** Last reported progress 0..=1 (best-effort). */
  progress: number;
}

const yieldTurn = () => new Promise<void>((r) => setTimeout(r, 0));

export class TranscriptionJob {
  private status: JobStatus = "pending";
  private progress = 0;
  private result: TranscriptionPlan | null = null;
  private failure: unknown = null;
  private cancelled = false;
  private started = false;
  private readonly done: Promise<TranscriptionPlan>;

  constructor(
    private readonly provider: TranscriptionProvider,
    private readonly input: TranscriptionInput,
    private readonly onProgress?: (p: number) => void,
  ) {
    this.done = this.run();
  }

  private report(p: number): void {
    this.progress = Math.max(0, Math.min(1, p));
    this.onProgress?.(this.progress);
  }

  private async run(): Promise<TranscriptionPlan> {
    this.started = true;
    this.status = "running";
    // Yield so the caller observes pending/running before completion.
    await yieldTurn();
    this.report(0.05);
    await yieldTurn();
    if (this.cancelled) {
      this.status = "cancelled";
      throw new Error("transcription cancelled");
    }
    try {
      const plan = await this.provider.transcribe(this.input);
      if (this.cancelled) {
        this.status = "cancelled";
        throw new Error("transcription cancelled");
      }
      this.result = plan;
      this.status = "done";
      this.report(1);
      return plan;
    } catch (e) {
      if (this.cancelled) this.status = "cancelled";
      else {
        this.status = "failed";
        this.failure = e;
      }
      throw e;
    }
  }

  /** Non-blocking snapshot: status plus last reported progress. */
  poll(): JobProgress {
    return { status: this.status, progress: this.progress };
  }

  /** Ask the job to stop early; the plan (if any) is discarded. */
  cancel(): void {
    if (this.status === "done" || this.status === "failed") return;
    this.cancelled = true;
    if (!this.started || this.status === "pending") this.status = "cancelled";
  }

  /** Await completion (throws on cancel / provider failure). */
  async wait(): Promise<TranscriptionPlan> {
    return this.done;
  }

  /** Finished plan, or null while pending/running/cancelled/failed. */
  tryTake(): TranscriptionPlan | null {
    return this.result;
  }

  failureCause(): unknown {
    return this.failure;
  }
}

/** Start a background transcription job (never blocks the caller). */
export function submitTranscription(
  provider: TranscriptionProvider,
  input: TranscriptionInput,
  onProgress?: (p: number) => void,
): TranscriptionJob {
  return new TranscriptionJob(provider, input, onProgress);
}
