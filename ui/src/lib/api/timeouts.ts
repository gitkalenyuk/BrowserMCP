// Per-call timeouts (`CallOptions.timeout`) of commands that answer only when long work is done.
// WsApi's default (15 s) suits quick commands; these wait for an AI model, a script or a
// transcription on the core. After a timeout the command keeps running there, so callers
// treat `isTimeout(err)` as "no answer yet", not as a failure.

/**
 * `run.heal_propose` / `run.heal_reject`: one AI call writes and validates a patch (an invalid
 * answer is sent back to the model, so it can take a few calls). The outcome also arrives as
 * `run.heal_proposed` / `run.heal_failed`, which callers wait for after a timeout.
 */
export const HEAL_AI_TIMEOUT_MS = 180_000;

/**
 * `voice.transcribe`: the cloud engines' HTTP limit is 180 s (crates/voice engines.rs), the local
 * engine may first start its server (up to 120 s) and then transcribe.
 */
export const VOICE_TRANSCRIBE_TIMEOUT_MS = 200_000;

/** `script.run` without `timeout_secs`: the core's default script timeout. */
export const DEFAULT_SCRIPT_TIMEOUT_SECS = 300;
/** The highest `script_timeout_secs` the core accepts (script.settings). */
export const MAX_SCRIPT_TIMEOUT_SECS = 86_400;
/** Extra wait after the script's own timeout: starting the script host, killing it, answering. */
export const SCRIPT_ANSWER_MARGIN_MS = 30_000;

/**
 * Timeout of a `script.run` call whose script may run `scriptTimeoutSecs` seconds (the core kills
 * it then). Unknown (`null`): the longest a script may be allowed to run.
 */
export function scriptRunTimeoutMs(scriptTimeoutSecs: number | null | undefined): number {
  const secs =
    typeof scriptTimeoutSecs === 'number' && Number.isFinite(scriptTimeoutSecs) && scriptTimeoutSecs > 0
      ? scriptTimeoutSecs
      : MAX_SCRIPT_TIMEOUT_SECS;
  return secs * 1000 + SCRIPT_ANSWER_MARGIN_MS;
}
