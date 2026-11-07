// Fake voice.* for MockApi (`pnpm dev`, tests). Mirrors crates/voice: settings with the same
// defaults and checks, keys never returned, engines ready / not ready, Local Whisper downloads
// with progress, streams whose interim text grows while audio arrives. No audio is sent
// anywhere: the "transcript" is a fixed sentence in the requested language.

import { RpcError } from './client';
import type { VoiceEngineId, VoiceLocalStatus, VoiceSettings, VoiceSettingsPatch } from './types-voice';
import { parseHotkey, formatHotkey, isPlain } from '../voice/keys';

const SENTENCES: Record<string, string> = {
  uk: 'Відкрий прогноз погоди для Києва і коротко його підсумуй',
  en: 'Open the weather forecast for Kyiv and sum it up briefly',
};

const MODELS = [
  { id: 'tiny', name: 'Tiny', size: 77_691_713, recommended: false, quality: 1, speed: 5 },
  { id: 'base', name: 'Base', size: 147_951_465, recommended: false, quality: 2, speed: 5 },
  { id: 'small', name: 'Small', size: 487_601_967, recommended: false, quality: 3, speed: 3 },
  { id: 'large-v3-turbo-q5_0', name: 'Large v3 Turbo (q5_0)', size: 574_041_195, recommended: true, quality: 4, speed: 3 },
  { id: 'medium-q5_0', name: 'Medium (q5_0)', size: 539_212_467, recommended: false, quality: 4, speed: 2 },
  { id: 'large-v3-turbo', name: 'Large v3 Turbo', size: 1_624_555_275, recommended: false, quality: 5, speed: 2 },
];
const PREFERRED = ['large-v3-turbo-q5_0', 'large-v3-turbo', 'medium-q5_0', 'small', 'base', 'tiny'];
const ENGINE_SIZE = 8_928_640;

const ENGINES: { id: VoiceEngineId; name: string; models: string[]; needsKey: boolean }[] = [
  { id: 'local', name: 'Local Whisper', models: [], needsKey: false },
  { id: 'openai', name: 'OpenAI', models: ['gpt-4o-transcribe', 'gpt-4o-mini-transcribe', 'whisper-1'], needsKey: true },
  { id: 'groq', name: 'Groq', models: ['whisper-large-v3-turbo', 'whisper-large-v3'], needsKey: true },
  { id: 'elevenlabs', name: 'ElevenLabs Scribe', models: ['scribe_v1'], needsKey: true },
  { id: 'deepgram', name: 'Deepgram', models: ['nova-3'], needsKey: true },
  { id: 'custom', name: 'Custom (OpenAI-compatible)', models: [], needsKey: false },
];
type Cloud = Exclude<VoiceEngineId, 'local'>;
const CLOUD: Cloud[] = ['openai', 'groq', 'elevenlabs', 'deepgram', 'custom'];

interface CloudState {
  api_key: string | null;
  model: string | null;
  base_url: string | null;
}

export function createVoiceMocks(opts: { aiKeys?: Partial<Record<'openai' | 'groq', boolean>> } = {}) {
  const s = {
    enabled: true,
    engine: 'local' as VoiceEngineId,
    mode: 'hold' as 'hold' | 'tap',
    hotkey: 'Space',
    auto_submit_hold: false,
    auto_submit_tap: true,
    language: '',
    page_hint: true,
    local: { model: null as string | null, threads: null as number | null, idle_minutes: 5 },
    cloud: Object.fromEntries(CLOUD.map((c) => [c, { api_key: null, model: null, base_url: null }])) as Record<Cloud, CloudState>,
  };
  const installed = new Set<string>(['engine', 'base']);
  let download: VoiceLocalStatus['download'] = null;
  let timer: ReturnType<typeof setInterval> | undefined;
  const streams = new Map<string, { engine: VoiceEngineId; ms: number; language: string }>();
  let nextStream = 1;
  const aiKey = (id: VoiceEngineId) => (id === 'openai' || id === 'groq') && !!opts.aiKeys?.[id];

  const view = (): VoiceSettings => {
    const cloud = (id: Cloud) => {
      const c = s.cloud[id];
      return {
        api_key_set: !!c.api_key,
        api_key_hint: c.api_key && c.api_key.length >= 8 ? `…${c.api_key.slice(-4)}` : null,
        ai_provider_key: aiKey(id),
        model: c.model,
        base_url: c.base_url,
      };
    };
    return {
      enabled: s.enabled,
      engine: s.engine,
      mode: s.mode,
      hotkey: s.hotkey,
      auto_submit_hold: s.auto_submit_hold,
      auto_submit_tap: s.auto_submit_tap,
      language: s.language,
      page_hint: s.page_hint,
      local: { ...s.local },
      openai: cloud('openai'),
      groq: cloud('groq'),
      elevenlabs: cloud('elevenlabs'),
      deepgram: cloud('deepgram'),
      custom: cloud('custom'),
    };
  };

  const activeModel = () => {
    if (s.local.model && installed.has(s.local.model)) return s.local.model;
    return PREFERRED.find((m) => installed.has(m)) ?? null;
  };

  const notReady = (id: VoiceEngineId): string | null => {
    if (id === 'local') {
      if (!installed.has('engine')) return 'the whisper.cpp engine is not downloaded';
      if (!activeModel()) return 'no Whisper model is downloaded';
      return null;
    }
    const c = s.cloud[id];
    if (id === 'custom') return c.base_url ? null : 'the custom engine has no API address';
    return c.api_key || aiKey(id) ? null : `${ENGINES.find((e) => e.id === id)!.name} has no API key`;
  };

  const status = (): VoiceLocalStatus => ({
    supported: true,
    engine: {
      installed: installed.has('engine'),
      version: installed.has('engine') ? 'b5454' : null,
      size: installed.has('engine') ? ENGINE_SIZE : null,
      folder: 'C:\\BrowserMCP\\data\\voice\\bin',
    },
    models: MODELS.map((m) => ({ ...m, file: `ggml-${m.id}.bin`, sha256: '0'.repeat(64), installed: installed.has(m.id) })),
    active_model: activeModel(),
    download: download ? { ...download } : null,
    server: null,
  });

  const requireReady = (id: VoiceEngineId) => {
    const why = notReady(id);
    if (why) throw new RpcError(id === 'local' ? 'NOT_FOUND' : 'INVALID_PARAMS', why, { hint: 'Set it up in Settings → Voice.' });
  };

  const lang = (l: string | null | undefined) => {
    const v = (l ?? s.language).trim().toLowerCase();
    return v === '' || v === 'auto' ? 'en' : v.split('-')[0];
  };
  const sentence = (l: string) => SENTENCES[l] ?? SENTENCES.en;
  /** The words heard after `ms` of audio (about 2.5 words a second). */
  const heard = (l: string, ms: number) => {
    const words = sentence(l).split(' ');
    return words.slice(0, Math.min(words.length, Math.floor(ms / 400))).join(' ');
  };

  function stopTimer() {
    clearInterval(timer);
    timer = undefined;
  }

  return {
    'voice.settings_get': () => view(),
    'voice.settings_set': (p: VoiceSettingsPatch) => {
      if (p.hotkey != null) {
        const h = parseHotkey(p.hotkey);
        if (!h || (isPlain(h) && h.key !== 'Space' && !/^F\d+$/.test(h.key))) {
          throw new RpcError('INVALID_PARAMS', `hotkey "${p.hotkey}" is not a key combination (examples: Space, Ctrl+Space, Alt+V, F8)`);
        }
      }
      if (p.language != null && p.language.trim() && p.language.trim() !== 'auto' && !/^[a-z]{2,3}([-_].*)?$/i.test(p.language.trim())) {
        throw new RpcError('INVALID_PARAMS', `language "${p.language}" is not a language code (use auto, uk, en, de…)`);
      }
      if (p.local?.model && !MODELS.some((m) => m.id === p.local!.model)) throw new RpcError('INVALID_PARAMS', `unknown Whisper model "${p.local.model}"`);
      if (p.local?.idle_minutes != null && (p.local.idle_minutes < 1 || p.local.idle_minutes > 120)) {
        throw new RpcError('INVALID_PARAMS', 'idle_minutes must be 1–120');
      }
      if (p.enabled != null) s.enabled = p.enabled;
      if (p.engine) s.engine = p.engine;
      if (p.mode) s.mode = p.mode;
      if (p.hotkey != null) s.hotkey = formatHotkey(parseHotkey(p.hotkey)!);
      if (p.auto_submit_hold != null) s.auto_submit_hold = p.auto_submit_hold;
      if (p.auto_submit_tap != null) s.auto_submit_tap = p.auto_submit_tap;
      if (p.language != null) s.language = p.language.trim() === '' ? '' : p.language.trim() === 'auto' ? 'auto' : p.language.trim().toLowerCase().split(/[-_]/)[0];
      if (p.page_hint != null) s.page_hint = p.page_hint;
      if (p.local) {
        if (p.local.model != null) s.local.model = p.local.model.trim() || null;
        if (p.local.threads != null) s.local.threads = p.local.threads > 0 ? p.local.threads : null;
        if (p.local.idle_minutes != null) s.local.idle_minutes = p.local.idle_minutes;
      }
      for (const id of CLOUD) {
        const patch = p[id];
        if (!patch) continue;
        const c = s.cloud[id];
        if (patch.api_key != null) c.api_key = patch.api_key.trim() || null;
        if (patch.model != null) c.model = patch.model.trim() || null;
        if (patch.base_url != null) {
          const u = patch.base_url.trim();
          if (u && !/^https?:\/\//.test(u)) throw new RpcError('INVALID_PARAMS', `base_url "${u}" must start with http:// or https://`);
          c.base_url = u ? u.replace(/\/+$/, '') : null;
        }
      }
      return view();
    },
    'voice.engines': () => ({
      active: s.engine,
      engines: ENGINES.map((e) => {
        const why = notReady(e.id);
        return {
          id: e.id,
          name: e.name,
          configured: !why,
          reason: why,
          needs_key: e.needsKey,
          key_source: e.id === 'local' ? null : s.cloud[e.id].api_key ? ('voice' as const) : aiKey(e.id) ? ('ai_provider' as const) : null,
          interim: e.id === 'local' || e.id === 'deepgram',
          models: e.models,
          model: e.id === 'local' ? activeModel() : (s.cloud[e.id].model ?? e.models[0] ?? null),
          page_hint: ['local', 'openai', 'groq', 'custom'].includes(e.id),
          offline: e.id === 'local',
        };
      }),
    }),
    'voice.transcribe': (p: { audio_base64: string; language?: string | null; engine?: VoiceEngineId | null }) => {
      if (!p.audio_base64) throw new RpcError('INVALID_PARAMS', 'audio_base64 is empty');
      const engine = p.engine ?? s.engine;
      requireReady(engine);
      const l = lang(p.language);
      return { text: sentence(l), language: l, duration_ms: 3000, engine, elapsed_ms: 420 };
    },
    'voice.stream_start': (p: { language?: string | null; engine?: VoiceEngineId | null }) => {
      if (!s.enabled) throw new RpcError('INVALID_PARAMS', 'voice dictation is turned off');
      const engine = p.engine ?? s.engine;
      requireReady(engine);
      const id = `v${nextStream++}`;
      streams.set(id, { engine, ms: 0, language: lang(p.language) });
      const interim = engine === 'local' || engine === 'deepgram';
      return { stream: id, engine, interim, live: engine === 'deepgram' };
    },
    'voice.stream_push': (p: { stream: string; pcm_base64: string }) => {
      const st = streams.get(p.stream);
      if (!st) throw new RpcError('NOT_FOUND', `no voice stream "${p.stream}"`);
      // base64 → bytes → 16-bit samples at 16 kHz.
      const bytes = Math.floor((p.pcm_base64.length * 3) / 4);
      st.ms += (bytes / 2 / 16_000) * 1000;
      const interim = st.engine === 'local' || st.engine === 'deepgram';
      return { partial: interim ? heard(st.language, st.ms) : '', duration_ms: Math.round(st.ms) };
    },
    'voice.stream_stop': (p: { stream: string; cancel?: boolean }) => {
      const st = streams.get(p.stream);
      if (!st) throw new RpcError('NOT_FOUND', `no voice stream "${p.stream}"`);
      streams.delete(p.stream);
      const text = p.cancel || st.ms < 300 ? '' : sentence(st.language);
      return { text, language: st.language, duration_ms: Math.round(st.ms), engine: st.engine, elapsed_ms: 300 };
    },
    'voice.local_status': () => status(),
    'voice.local_download': ({ what }: { what: string }) => {
      if (what !== 'engine' && !MODELS.some((m) => m.id === what)) throw new RpcError('INVALID_PARAMS', `unknown download "${what}"`);
      if (download && ['running', 'verifying', 'installing'].includes(download.phase)) {
        throw new RpcError('INVALID_PARAMS', `${download.what} is still downloading`);
      }
      const total = what === 'engine' ? ENGINE_SIZE : MODELS.find((m) => m.id === what)!.size;
      download = { what, phase: 'running', received: 0, total, error: null };
      stopTimer();
      timer = setInterval(() => {
        if (!download || download.phase !== 'running') return stopTimer();
        download.received = Math.min(total, download.received + Math.ceil(total / 12));
        if (download.received >= total) {
          download.phase = 'done';
          installed.add(what);
          stopTimer();
        }
      }, 250);
      return status();
    },
    'voice.local_cancel': () => {
      if (download && download.phase === 'running') download.phase = 'cancelled';
      stopTimer();
      return status();
    },
    'voice.local_delete': ({ what }: { what: string }) => {
      installed.delete(what);
      return status();
    },
  };
}
