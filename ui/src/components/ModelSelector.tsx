import { useState, useEffect, useRef, useMemo } from 'react';
import { fetchModelsForProvider, matchesModality, formatContext, formatPrice } from '../api/modelProviders';
import type { ModelInfo } from '../api/modelProviders';

interface Props {
  /** Provider ID to fetch models for */
  provider: string;
  /** Currently selected model ID (empty string for "add model" mode) */
  value: string;
  /** Called when a model is selected */
  onChange: (modelId: string) => void;
  /** Modality filter pattern for category-based filtering */
  modality?: string;
}

export default function ModelSelector({ provider, value, onChange, modality }: Props) {
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState('');
  const [open, setOpen] = useState(false);
  const [highlighted, setHighlighted] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  // Fetch models when provider changes
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    fetchModelsForProvider(provider).then((result) => {
      if (!cancelled) {
        setModels(result);
        setLoading(false);
      }
    });
    return () => { cancelled = true; };
  }, [provider]);

  // Filter by modality then search text
  const filtered = useMemo(() => {
    let result = models;
    if (modality) {
      result = result.filter((m) => matchesModality(m, modality));
    }
    if (search) {
      const q = search.toLowerCase();
      result = result.filter((m) =>
        m.id.toLowerCase().includes(q) || m.name.toLowerCase().includes(q)
      );
    }
    return result.slice(0, 60);
  }, [models, search, modality]);

  const selected = value ? models.find((m) => m.id === value) : null;

  const handleSelect = (modelId: string) => {
    onChange(modelId);
    setSearch('');
    setOpen(false);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setHighlighted((h) => Math.min(h + 1, filtered.length - 1));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setHighlighted((h) => Math.max(h - 1, 0));
    } else if (e.key === 'Enter' && filtered[highlighted]) {
      e.preventDefault();
      handleSelect(filtered[highlighted].id);
    } else if (e.key === 'Escape') {
      setOpen(false);
    }
  };

  // Scroll highlighted into view
  useEffect(() => {
    if (open && listRef.current) {
      const el = listRef.current.children[highlighted] as HTMLElement;
      el?.scrollIntoView({ block: 'nearest' });
    }
  }, [highlighted, open]);

  // Reset highlighted when filter changes
  useEffect(() => { setHighlighted(0); }, [provider, search]);

  return (
    <div className="relative">
      <label className="block text-xs text-gray-500 mb-1">
        Model
        {loading && <span className="text-gray-400 ml-1">(loading...)</span>}
        {!loading && <span className="text-gray-400 ml-1">({filtered.length} available)</span>}
      </label>

      {/* Selected model display (only when value is set and dropdown is closed) */}
      {selected && !open && (
        <button
          type="button"
          onClick={() => { setOpen(true); setSearch(''); setTimeout(() => inputRef.current?.focus(), 0); }}
          className="w-full text-left px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white hover:border-[var(--color-accent)] transition-colors"
        >
          <div className="flex items-center justify-between">
            <div className="min-w-0">
              <span className="font-medium truncate">{selected.name}</span>
              <span className="text-gray-400 ml-2 font-mono text-xs">{selected.id}</span>
            </div>
            <div className="flex items-center gap-2 text-xs text-gray-500 shrink-0">
              {selected.context_length > 0 && <span>{formatContext(selected.context_length)} ctx</span>}
              <span>{formatPrice(selected.pricing.prompt)} in</span>
            </div>
          </div>
        </button>
      )}

      {/* Search input (shown when no value or dropdown is open) */}
      {(!selected || open) && (
        <input
          ref={inputRef}
          type="text"
          value={search}
          onChange={(e) => { setSearch(e.target.value); setOpen(true); setHighlighted(0); }}
          onFocus={() => setOpen(true)}
          onKeyDown={handleKeyDown}
          placeholder={value ? 'Search models...' : 'Search and add a model...'}
          className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
        />
      )}

      {/* Dropdown */}
      {open && (
        <>
          <div className="fixed inset-0 z-10" onClick={() => setOpen(false)} />
          <div
            ref={listRef}
            className="absolute z-20 mt-1 w-full max-h-72 overflow-auto bg-white border border-gray-200 rounded-lg shadow-lg"
          >
            {filtered.length === 0 ? (
              <div className="px-4 py-3 text-sm text-gray-400">
                No models found{modality ? ' for this modality' : ''}
              </div>
            ) : (
              filtered.map((model, i) => (
                <button
                  key={model.id}
                  type="button"
                  onClick={() => handleSelect(model.id)}
                  className={`w-full text-left px-3 py-2 text-sm border-b border-gray-50 transition-colors ${
                    i === highlighted ? 'bg-blue-50' : 'hover:bg-gray-50'
                  } ${model.id === value ? 'bg-blue-50' : ''}`}
                >
                  <div className="flex items-center justify-between">
                    <div className="min-w-0">
                      <div className="truncate font-medium">{model.name}</div>
                      <div className="text-xs text-gray-400 font-mono truncate">{model.id}</div>
                    </div>
                    <div className="flex items-center gap-2 text-xs text-gray-500 flex-shrink-0 ml-2">
                      {model.context_length > 0 && (
                        <span className="bg-gray-100 px-1.5 py-0.5 rounded">{formatContext(model.context_length)}</span>
                      )}
                      <span>{formatPrice(model.pricing.prompt)}</span>
                    </div>
                  </div>
                </button>
              ))
            )}
          </div>
        </>
      )}

      {/* Manual entry option */}
      {!open && !value && (
        <div className="mt-1">
          <button
            type="button"
            onClick={() => {
              const custom = prompt('Enter model ID:');
              if (custom) onChange(custom);
            }}
            className="text-xs text-[var(--color-accent)] hover:underline"
          >
            Enter custom model ID
          </button>
        </div>
      )}
    </div>
  );
}
