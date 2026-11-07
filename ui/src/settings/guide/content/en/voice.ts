// Guide, English: voice dictation. Facts: crates/voice (lib.rs, local.rs, engines.rs),
// ui/src/lib/voice, ui/src/lib/components/voice/VoiceInput.svelte, ui/src/settings/VoiceSection.svelte.
import { api, chapter, key, keys, list, p, section, settings, steps, terms, tip, warning } from '../../model';

export const voice = chapter(
  'voice',
  'Voice dictation',
  'Speak instead of typing in the task box and the AI chat: hold Space or click the microphone; local Whisper (offline, free) or a cloud engine.',
  section(
    'what',
    'What it is',
    p(
      'Voice dictation turns speech into text right in the prompt, like `/voice` in Claude Code. The text appears where the cursor is, while you speak (dimmed until it is final), and you can keep typing around it. It works in the task box of the new tab page (**What should I do?**) and in the AI chat.',
    ),
    p(
      'The speech is recognized by an **engine** you choose: **Local Whisper** runs on your computer (nothing leaves it, nothing to pay), or a cloud service with your own key: OpenAI, Groq, ElevenLabs, Deepgram or any OpenAI-compatible server.',
    ),
  ),
  section(
    'why',
    'What it is for',
    list(
      'Long tasks for an agent are quicker to say than to type: “Find three flats in Lviv under 15 thousand with a balcony and open them in tabs.”',
      'Ukrainian and other languages: with **Large v3 Turbo** Whisper understands them well, offline.',
      'Hands busy, or typing is hard: hold one key and talk.',
    ),
  ),
  section(
    'start',
    'Set it up (once)',
    steps(
      'Open **Settings → Voice**.',
      'Keep **Local Whisper** as the engine, click **Download** next to the **whisper.cpp engine** (about 9 MB) and then next to a model: **Base** (141 MB) for a quick start, **Large v3 Turbo (q5_0)** (547 MB) for Ukrainian.',
      'Click **Test (3 s)**, say a sentence, and check the text that comes back.',
    ),
    p(
      'Nothing is downloaded until you click **Download**. The engine comes from the official whisper.cpp releases on GitHub (`ggml-org/whisper.cpp`), the models from the official Hugging Face repository (`ggerganov/whisper.cpp`); each file is checked against the size and SHA-256 its source publishes, and a mismatch is thrown away. **Cancel** stops a download, the cross next to a model deletes it.',
    ),
    settings('voice', 'Engine, keys, local models, hotkey and the test (the “Voice” section).'),
  ),
  section(
    'use',
    'Dictating',
    terms(
      ['Hold mode (default)', 'Click into an empty prompt and hold [[Space]]. After a moment “keep holding…” turns into “listening…”; speak; let go to finish. A short tap of Space still types a space.'],
      ['Tap mode', 'Press the hotkey once to start (“● REC · tap to send”) and once more to stop. Choose it under **Mode**.'],
      ['Microphone button', 'Next to the prompt. Click = tap mode (click again to stop); press and hold = hold mode (release to finish).'],
      ['Another hotkey', 'Under **Hotkey**, click the field and press the combination, for example [[Ctrl+Space]] or [[Alt+V]]. Combinations start at once, without the warm-up, also when the prompt has text.'],
    ),
    keys(
      key('Space', 'Hold on an empty prompt: dictate (hold mode)'),
      key('Esc', 'Cancel: stop the microphone, discard the text, put the prompt back as it was'),
      key('Ctrl+C', 'Cancel too (when no text is selected)'),
    ),
    list(
      'The ring around the microphone and the small bar next to “listening…” rise and fall with your voice: if they stay still, the wrong microphone is in use (Windows sound settings).',
      'Dictation stops by itself after **15 s of silence** or after **2 minutes**.',
      '**Auto-submit** sends the prompt when you finish, if the text has at least three words: on by default in tap mode, off in hold mode.',
      'After three errors within 10 seconds dictation pauses for 10 seconds and says why, instead of failing on every key press.',
    ),
    tip('**Language**: by default the interface language. Pick **Detect automatically** if you switch languages, or a fixed one for the best accuracy. **Page hint** passes the current page’s title and site to the engines that take a hint, so names on the page are spelled right.'),
  ),
  section(
    'engines',
    'Engines',
    terms(
      ['Local Whisper', 'whisper.cpp on your computer: offline, free, private. The text appears while you speak (the recording is re-read about every 1.5 s). The helper program starts with the first dictation and stops after a few idle minutes (**Stop after idle**) and when the browser closes.'],
      ['OpenAI', '`gpt-4o-transcribe`, `gpt-4o-mini-transcribe` or `whisper-1`. Uses the key from **Settings → AI** (OpenAI) when Voice has none of its own.'],
      ['Groq', '`whisper-large-v3-turbo` or `whisper-large-v3`, very fast. Also reuses the Groq key from Settings → AI.'],
      ['ElevenLabs Scribe', '`scribe_v1`, with its own key.'],
      ['Deepgram', '`nova-3`, with its own key. Live: the text streams in while you speak.'],
      ['Custom', 'Any server with an OpenAI-compatible `/audio/transcriptions` (a self-hosted faster-whisper, a company gateway): base URL, optional key, model.'],
    ),
    p(
      'Paid engines that work on whole recordings (OpenAI, Groq, ElevenLabs, custom) get the audio once, when you stop: while you speak you see the level meter, then “transcribing…”, then the text. Sending the growing recording again and again for a live preview would multiply the cost.',
    ),
    api('voice.engines', {}, 'Which engines are ready, live or offline.'),
    api('voice.transcribe', { audio_base64: 'UklGRiQAAABXQVZFZm10IBAAAAABAAEAgD4AAAB9AAACABAAZGF0YQAAAAA=', language: 'uk' }, 'Transcribe a recording (base64 WAV) with the chosen engine.'),
    api('voice.local_download', { what: 'large-v3-turbo-q5_0' }, 'Download a Whisper model (the same as the Download button).'),
    warning('Keys are stored in plain text in `data\\config\\voice.json`, like the AI provider keys. With a cloud engine your recording goes to that company.'),
  ),
  section(
    'limits',
    'Limits',
    list(
      'The microphone is used only by the browser’s own pages (the new tab page, the AI panel, Settings); websites still ask for it through the permission bar as usual.',
      'Local Whisper needs Windows on x64; models run on the CPU (the CPU build). Large models need about 1–2 GB of memory while the helper runs.',
      'With Local Whisper the live text is re-read from the start every time, so on a slow computer it lags behind long dictations; the final text is always complete.',
      'There is no live preview with OpenAI, Groq, ElevenLabs and custom engines (see above).',
      'The dimmed preview is drawn over the prompt; in an unusually styled field it may sit slightly off while you speak. The final text is plain text.',
      'The omnibox (address bar) has no dictation.',
    ),
  ),
);
