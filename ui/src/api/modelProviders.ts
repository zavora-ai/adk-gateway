/**
 * Provider-specific model fetching.
 *
 * OpenRouter models are fetched from the API and cached.
 * Specialty providers (ElevenLabs, Replicate, AssemblyAI, Deepgram) use hardcoded lists.
 * Ollama fetches from localhost.
 */

export interface ModelInfo {
  id: string;
  name: string;
  context_length: number;
  pricing: { prompt: string; completion: string };
  modality?: string;
}

// ── OpenRouter (cached) ────────────────────────────────────────────

let openRouterCache: ModelInfo[] | null = null;
let openRouterFetchPromise: Promise<ModelInfo[]> | null = null;

async function fetchOpenRouterModels(): Promise<ModelInfo[]> {
  if (openRouterCache) return openRouterCache;
  if (openRouterFetchPromise) return openRouterFetchPromise;

  openRouterFetchPromise = fetch('https://openrouter.ai/api/v1/models')
    .then((r) => r.json())
    .then((data) => {
      if (data.data?.length > 0) {
        openRouterCache = data.data.map((m: Record<string, unknown>) => ({
          id: m.id as string,
          name: m.name as string,
          context_length: (m.context_length as number) || 0,
          pricing: (m.pricing as { prompt: string; completion: string }) || { prompt: '0', completion: '0' },
          modality: (m.architecture as { modality?: string })?.modality || '',
        }));
      } else {
        openRouterCache = FALLBACK_MODELS;
      }
      return openRouterCache!;
    })
    .catch(() => {
      openRouterCache = FALLBACK_MODELS;
      return openRouterCache;
    });

  return openRouterFetchPromise;
}

// ── Ollama (local) ─────────────────────────────────────────────────

async function fetchOllamaModels(): Promise<ModelInfo[]> {
  try {
    const res = await fetch('http://localhost:11434/api/tags', { signal: AbortSignal.timeout(3000) });
    const data = await res.json();
    if (data.models?.length > 0) {
      return data.models.map((m: { name: string; size: number }) => ({
        id: `ollama/${m.name}`,
        name: m.name,
        context_length: 0,
        pricing: { prompt: '0', completion: '0' },
        modality: 'text->text',
      }));
    }
  } catch {
    // Ollama not running — return empty
  }
  return [];
}

// ── Hardcoded specialty providers ──────────────────────────────────

const ELEVENLABS_MODELS: ModelInfo[] = [
  { id: 'elevenlabs/eleven_turbo_v2_5', name: 'Turbo v2.5', context_length: 0, pricing: { prompt: '0', completion: '0' }, modality: 'text->audio' },
  { id: 'elevenlabs/eleven_multilingual_v2', name: 'Multilingual v2', context_length: 0, pricing: { prompt: '0', completion: '0' }, modality: 'text->audio' },
  { id: 'elevenlabs/eleven_flash_v2_5', name: 'Flash v2.5', context_length: 0, pricing: { prompt: '0', completion: '0' }, modality: 'text->audio' },
];

const REPLICATE_MODELS: ModelInfo[] = [
  { id: 'replicate/flux-pro', name: 'FLUX Pro', context_length: 0, pricing: { prompt: '0', completion: '0' }, modality: 'text->image' },
  { id: 'replicate/flux-schnell', name: 'FLUX Schnell', context_length: 0, pricing: { prompt: '0', completion: '0' }, modality: 'text->image' },
  { id: 'replicate/sdxl', name: 'Stable Diffusion XL', context_length: 0, pricing: { prompt: '0', completion: '0' }, modality: 'text->image' },
];

const ASSEMBLYAI_MODELS: ModelInfo[] = [
  { id: 'assemblyai/best', name: 'Best (highest accuracy)', context_length: 0, pricing: { prompt: '0', completion: '0' }, modality: 'audio->text' },
  { id: 'assemblyai/nano', name: 'Nano (fastest)', context_length: 0, pricing: { prompt: '0', completion: '0' }, modality: 'audio->text' },
];

const DEEPGRAM_MODELS: ModelInfo[] = [
  { id: 'deepgram/nova-3', name: 'Nova 3', context_length: 0, pricing: { prompt: '0', completion: '0' }, modality: 'audio->text' },
  { id: 'deepgram/nova-2', name: 'Nova 2', context_length: 0, pricing: { prompt: '0', completion: '0' }, modality: 'audio->text' },
  { id: 'deepgram/whisper-large', name: 'Whisper Large', context_length: 0, pricing: { prompt: '0', completion: '0' }, modality: 'audio->text' },
];

// ── Fallback models when OpenRouter is unreachable ─────────────────

const FALLBACK_MODELS: ModelInfo[] = [
  { id: 'google/gemini-2.5-flash', name: 'Gemini 2.5 Flash', context_length: 1048576, pricing: { prompt: '0.0000015', completion: '0.000006' }, modality: 'text+image->text' },
  { id: 'google/gemini-2.5-pro', name: 'Gemini 2.5 Pro', context_length: 1048576, pricing: { prompt: '0.00000625', completion: '0.000025' }, modality: 'text+image->text' },
  { id: 'anthropic/claude-sonnet-4', name: 'Claude Sonnet 4', context_length: 200000, pricing: { prompt: '0.000003', completion: '0.000015' }, modality: 'text+image->text' },
  { id: 'anthropic/claude-opus-4', name: 'Claude Opus 4', context_length: 200000, pricing: { prompt: '0.000015', completion: '0.000075' }, modality: 'text+image->text' },
  { id: 'openai/gpt-4o', name: 'GPT-4o', context_length: 128000, pricing: { prompt: '0.0000025', completion: '0.00001' }, modality: 'text+image->text' },
  { id: 'openai/gpt-4o-mini', name: 'GPT-4o Mini', context_length: 128000, pricing: { prompt: '0.00000015', completion: '0.0000006' }, modality: 'text+image->text' },
  { id: 'deepseek/deepseek-chat-v3', name: 'DeepSeek V3', context_length: 131072, pricing: { prompt: '0.0000003', completion: '0.0000009' }, modality: 'text->text' },
  { id: 'meta-llama/llama-4-maverick', name: 'Llama 4 Maverick', context_length: 1048576, pricing: { prompt: '0.0000002', completion: '0.0000008' }, modality: 'text+image->text' },
  { id: 'mistralai/mistral-large', name: 'Mistral Large', context_length: 128000, pricing: { prompt: '0.000002', completion: '0.000006' }, modality: 'text->text' },
];

// ── Provider prefix mapping ────────────────────────────────────────

const PROVIDER_PREFIXES: Record<string, string[]> = {
  'gemini': ['google/'],
  'anthropic': ['anthropic/'],
  'openai': ['openai/'],
  'ollama': ['ollama/'],
  'deepseek': ['deepseek/'],
  'groq': ['groq/'],
  'openrouter': [], // show all
  'fireworks': ['fireworks/'],
  'together': ['together/'],
  'mistral': ['mistralai/', 'mistral/'],
  'perplexity': ['perplexity/'],
  'cerebras': ['cerebras/'],
  'sambanova': ['sambanova/'],
  'xai': ['x-ai/', 'xai/'],
  'elevenlabs': ['elevenlabs/'],
  'replicate': ['replicate/'],
  'assemblyai': ['assemblyai/'],
  'deepgram': ['deepgram/'],
};

// ── Public API ─────────────────────────────────────────────────────

/** Fetch models for a given provider. Returns cached results when available. */
export async function fetchModelsForProvider(providerId: string): Promise<ModelInfo[]> {
  switch (providerId) {
    case 'elevenlabs':
      return ELEVENLABS_MODELS;
    case 'replicate':
      return REPLICATE_MODELS;
    case 'assemblyai':
      return ASSEMBLYAI_MODELS;
    case 'deepgram':
      return DEEPGRAM_MODELS;
    case 'ollama':
      return fetchOllamaModels();
    default: {
      // All other providers use OpenRouter's model list, filtered by prefix
      const all = await fetchOpenRouterModels();
      const prefixes = PROVIDER_PREFIXES[providerId] || [`${providerId}/`];
      if (prefixes.length === 0) return all; // openrouter = show all
      return all.filter((m) =>
        prefixes.some((p) => m.id.toLowerCase().startsWith(p))
      );
    }
  }
}

/** Known music generation model patterns — excluded from TTS, included in Music */
const MUSIC_MODEL_PATTERNS = ['lyria', 'music', 'jukebox', 'musicgen', 'audiogen'];

function isMusicModel(model: ModelInfo): boolean {
  const id = model.id.toLowerCase();
  const name = model.name.toLowerCase();
  return MUSIC_MODEL_PATTERNS.some(p => id.includes(p) || name.includes(p));
}

/** Check if a model's modality matches a category requirement */
export function matchesModality(model: ModelInfo, modality: string): boolean {
  const m = (model.modality || '').toLowerCase();
  if (!m) return false;
  switch (modality) {
    case 'text->text':
      return m.includes('text');
    case 'text+image->text':
      return m.includes('image') && (m.split('->')[0]?.includes('image') ?? false);
    case 'text->image':
      return m.split('->')[1]?.includes('image') || false;
    case 'text->audio':
      // TTS: output contains audio, but exclude music generation models
      if (isMusicModel(model)) return false;
      return m.split('->')[1]?.includes('audio') || false;
    case 'text->music':
      // Music generation: only known music models
      return isMusicModel(model);
    case 'audio->text':
      return m.split('->')[0]?.includes('audio') || false;
    case 'embedding':
      return m.includes('embedding');
    default:
      return true;
  }
}

/** Format context length for display */
export function formatContext(len: number): string {
  if (len >= 1000000) return `${(len / 1000000).toFixed(1)}M`;
  if (len >= 1000) return `${Math.round(len / 1000)}K`;
  if (len === 0) return '—';
  return `${len}`;
}

/** Format price per token for display */
export function formatPrice(perToken: string): string {
  const val = parseFloat(perToken);
  if (val === 0) return 'Free';
  const perMillion = val * 1000000;
  if (perMillion < 0.01) return '<$0.01/M';
  if (perMillion < 1) return `$${perMillion.toFixed(2)}/M`;
  return `$${perMillion.toFixed(1)}/M`;
}

/** Extract provider ID from a model ID like "anthropic/claude-sonnet-4" */
export function extractProvider(modelId: string): string {
  if (!modelId || !modelId.includes('/')) return '';
  const prefix = modelId.split('/')[0].toLowerCase();
  const map: Record<string, string> = {
    'google': 'gemini',
    'mistralai': 'mistral',
    'x-ai': 'xai',
  };
  return map[prefix] || prefix;
}
