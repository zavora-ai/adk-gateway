/**
 * Preset profiles for quick model configuration.
 *
 * Each preset pre-fills all model categories with recommended models.
 * Users can customize any category after selecting a preset.
 */

export interface PresetCategory {
  /** Ordered list of model IDs (first = primary, rest = fallbacks) */
  models: string[];
}

export interface PresetProfile {
  id: string;
  icon: string;
  name: string;
  description: string;
  estimatedCost: string;
  categories: Record<string, PresetCategory>;
  /** Enterprise presets don't have pre-filled models */
  isEnterprise?: boolean;
}

export type CloudProvider = 'azure' | 'gcp' | 'bedrock';

export interface CloudConfig {
  provider: CloudProvider;
  /** Azure: endpoint URL, GCP: project ID, Bedrock: region */
  endpoint: string;
  /** Azure: API version, GCP: region, Bedrock: profile */
  secondary: string;
  /** Azure: deployment name, GCP: model name, Bedrock: model ARN */
  modelId: string;
}

export const PRESETS: PresetProfile[] = [
  {
    id: 'free',
    icon: '🆓',
    name: 'Free Tier',
    description: 'Zero-cost models via OpenRouter free tier. Great for experimentation and development.',
    estimatedCost: '$0/month',
    categories: {
      primary: { models: ['google/gemma-4-31b-it:free'] },
      code: { models: ['nvidia/nemotron-3-super-120b-a12b:free'] },
      vision: { models: ['nvidia/nemotron-nano-12b-v2-vl:free'] },
      omni: { models: ['google/gemma-4-26b-a4b-it:free'] },
      tts: { models: [] },
      stt: { models: [] },
      music: { models: ['google/lyria-3-pro-preview'] },
      image_generation: { models: [] },
      embedding: { models: [] },
      search: { models: ['minimax/minimax-m2.5:free'] },
    },
  },
  {
    id: 'frontier',
    icon: '🚀',
    name: 'Frontier',
    description: 'Best-in-class models for each category. Maximum quality, higher cost.',
    estimatedCost: '~$5-15/M tokens',
    categories: {
      primary: { models: ['openai/gpt-5.5'] },
      code: { models: ['anthropic/claude-opus-4.7'] },
      vision: { models: ['google/gemini-3.1-pro-preview'] },
      omni: { models: ['google/gemini-3.1-pro-preview'] },
      tts: { models: ['google/gemini-3.1-flash-tts-preview'] },
      stt: { models: ['mistralai/voxtral-small-24b-2507'] },
      music: { models: ['google/lyria-3-pro-preview'] },
      image_generation: { models: ['google/gemini-3.1-flash-image-preview'] },
      embedding: { models: [] },
      search: { models: ['openai/gpt-5.5'] },
    },
  },
  {
    id: 'auto',
    icon: '⚡',
    name: 'Auto Intelligence',
    description: 'Smart routing: cheap models for common tasks, frontier fallbacks for complex ones.',
    estimatedCost: '~$0.10-2/M tokens',
    categories: {
      primary: { models: ['gemini-3.1-flash-lite', 'openai/gpt-5.5'] },
      code: { models: ['openai/gpt-5.5', 'anthropic/claude-opus-4.7'] },
      vision: { models: ['gemini-3.1-flash-lite', 'google/gemini-3.1-pro-preview'] },
      omni: { models: ['google/gemini-3.1-pro-preview', 'google/gemini-2.5-pro'] },
      tts: { models: ['google/gemini-3.1-flash-tts-preview'] },
      stt: { models: ['mistralai/voxtral-small-24b-2507'] },
      music: { models: ['google/lyria-2'] },
      image_generation: { models: ['google/gemini-2.5-flash-image', 'google/gemini-3.1-flash-image-preview'] },
      embedding: { models: [] },
      search: { models: ['google/gemini-2.5-flash', 'openai/gpt-5.5'] },
    },
  },
  {
    id: 'enterprise',
    icon: '🏢',
    name: 'Enterprise',
    description: 'Use your own Azure OpenAI, Google Vertex AI, or AWS Bedrock deployments.',
    estimatedCost: 'Varies by contract',
    isEnterprise: true,
    categories: {
      primary: { models: [] },
      code: { models: [] },
      vision: { models: [] },
      omni: { models: [] },
      tts: { models: [] },
      stt: { models: [] },
      music: { models: [] },
      image_generation: { models: [] },
      embedding: { models: [] },
      search: { models: [] },
    },
  },
];

export const CLOUD_PROVIDERS: { id: CloudProvider; name: string; description: string }[] = [
  {
    id: 'azure',
    name: 'Azure OpenAI',
    description: 'Requires endpoint URL, API version, and deployment name per model.',
  },
  {
    id: 'gcp',
    name: 'Google Vertex AI',
    description: 'Requires project ID, region, and model name.',
  },
  {
    id: 'bedrock',
    name: 'AWS Bedrock',
    description: 'Requires region, AWS profile, and model ARN.',
  },
];

/** Check if a current config matches a preset */
export function detectPreset(categories: Record<string, string[]>): string | null {
  for (const preset of PRESETS) {
    if (preset.isEnterprise) continue;
    let matches = true;
    for (const [key, cat] of Object.entries(preset.categories)) {
      const current = categories[key] || [];
      if (cat.models.length !== current.length) { matches = false; break; }
      if (!cat.models.every((m, i) => m === current[i])) { matches = false; break; }
    }
    if (matches) return preset.id;
  }
  return null;
}
