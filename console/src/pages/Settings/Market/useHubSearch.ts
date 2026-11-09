import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { marketApi } from "../../../api/modules/market";
import type {
  MarketProviderInfo,
  MarketResult,
  MarketSearchError,
  MarketSearchResponse,
  ProviderPageInfo,
} from "../../../api/modules/market";

const PER_PROVIDER_LIMIT = 10;
const STORAGE_KEY = "rua:market:selectedProviders";

export type SortBy = "relevance" | "downloads" | "installs" | "stars";

interface PageCache {
  results: MarketResult[];
  errors: MarketSearchError[];
  byProvider: Record<string, ProviderPageInfo>;
}

export interface HubSearchState {
  providers: MarketProviderInfo[];
  selectedProviderKeys: Set<string>;
  toggleProvider: (key: string) => void;
  query: string;
  setQuery: (q: string) => void;
  triggerSearch: () => void;
  results: MarketResult[];
  displayResults: MarketResult[];
  errors: MarketSearchError[];
  globalError: string | null;
  loading: boolean;
  currentPage: number;
  totalPages: number;
  totalResults: number;
  goToPage: (page: number) => void;
  retry: () => void;
  refreshProviders: () => void;
  sortBy: SortBy;
  setSortBy: (s: SortBy) => void;
  filterCategory: string;
  setFilterCategory: (c: string) => void;
  categories: string[];
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export function useHubSearch(): HubSearchState {
  const { i18n } = useTranslation();
  const lang = i18n.language || "en";
  const [providers, setProviders] = useState<MarketProviderInfo[]>([]);
  const [selectedProviderKeys, setSelectedProviderKeys] = useState<Set<string>>(
    new Set(),
  );
  const [sortBy, setSortBy] = useState<SortBy>("relevance");
  const [filterCategory, setFilterCategory] = useState("");
  const [query, setQueryState] = useState("");
  const [results, setResults] = useState<MarketResult[]>([]);
  const [errors, setErrors] = useState<MarketSearchError[]>([]);
  const [globalError, setGlobalError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [currentPage, setCurrentPage] = useState(1);
  const [byProvider, setByProvider] = useState<
    Record<string, ProviderPageInfo>
  >({});
  const requestSeqRef = useRef(0);
  const pageCacheRef = useRef<Map<number, PageCache>>(new Map());
  const prefetchAbortRef = useRef<AbortController | null>(null);
  const [cacheVersion, setCacheVersion] = useState(0);

  const providerKeyList = useMemo(
    () => Array.from(selectedProviderKeys).sort(),
    [selectedProviderKeys],
  );

  const sortedPageSize = useMemo(() => {
    void cacheVersion;
    const page1 = pageCacheRef.current.get(1);
    return page1 && page1.results.length > 0
      ? page1.results.length
      : PER_PROVIDER_LIMIT;
  }, [cacheVersion]);

  const sortedFilteredResults = useMemo(() => {
    if (sortBy === "relevance") return null;
    void cacheVersion;
    const all: MarketResult[] = [];
    const pages = Array.from(pageCacheRef.current.entries()).sort(
      ([a], [b]) => a - b,
    );
    for (const [, cache] of pages) {
      all.push(...cache.results);
    }
    let sorted = [...all].sort((a, b) => {
      const av = Number(a.stats?.[sortBy] ?? -1);
      const bv = Number(b.stats?.[sortBy] ?? -1);
      if (av < 0 && bv < 0) return 0;
      if (av < 0) return 1;
      if (bv < 0) return -1;
      return bv - av;
    });
    if (filterCategory) {
      sorted = sorted.filter((r) => {
        const cat = r.stats?.category;
        return typeof cat === "string" && cat === filterCategory;
      });
    }
    return sorted;
  }, [sortBy, filterCategory, cacheVersion]);

  const totalResults = useMemo(() => {
    if (sortedFilteredResults) return sortedFilteredResults.length;
    return Object.values(byProvider).reduce((sum, p) => sum + p.total, 0);
  }, [byProvider, sortedFilteredResults]);

  const totalPages = useMemo(() => {
    if (sortedFilteredResults) {
      if (sortedFilteredResults.length === 0) return 0;
      return Math.ceil(sortedFilteredResults.length / sortedPageSize);
    }
    const totals = Object.values(byProvider);
    if (totals.length === 0) return 0;
    return Math.max(
      ...totals.map((p) => Math.ceil(p.total / PER_PROVIDER_LIMIT)),
    );
  }, [byProvider, sortedFilteredResults, sortedPageSize]);

  const providersSeqRef = useRef(0);
  const fetchProviders = useCallback(() => {
    const seq = ++providersSeqRef.current;
    setGlobalError(null);
    setLoading(true);
    marketApi
      .listMarketProviders()
      .then((list) => {
        if (seq !== providersSeqRef.current) return;
        setProviders(list);
        const enabled = list.filter((p) => p.available).map((p) => p.key);
        const availableSet = new Set(enabled);
        let selection: Set<string> | null = null;
        try {
          const saved = localStorage.getItem(STORAGE_KEY);
          if (saved !== null) {
            const parsed: unknown = JSON.parse(saved);
            if (Array.isArray(parsed)) {
              const restored = parsed.filter(
                (k): k is string =>
                  typeof k === "string" && availableSet.has(k),
              );
              selection = new Set(restored);
              localStorage.setItem(STORAGE_KEY, JSON.stringify(restored));
            }
          }
        } catch {}
        setSelectedProviderKeys(selection ?? new Set(enabled));
      })
      .catch((err: unknown) => {
        if (seq !== providersSeqRef.current) return;
        setProviders([]);
        setGlobalError(errorMessage(err));
      })
      .finally(() => {
        if (seq === providersSeqRef.current) setLoading(false);
      });
  }, []);

  useEffect(() => {
    fetchProviders();
  }, [fetchProviders]);

  const toggleProvider = useCallback((key: string) => {
    setSelectedProviderKeys((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      try {
        localStorage.setItem(STORAGE_KEY, JSON.stringify(Array.from(next)));
      } catch {}
      return next;
    });
  }, []);

  const applyResponse = useCallback(
    (resp: MarketSearchResponse, page: number) => {
      setResults(resp.results);
      setErrors(resp.errors);
      setByProvider(resp.by_provider);
      setCurrentPage(page);
      pageCacheRef.current.set(page, {
        results: resp.results,
        errors: resp.errors,
        byProvider: resp.by_provider,
      });
      setCacheVersion((v) => v + 1);
    },
    [],
  );

  const prefetchAllPages = useCallback(
    async (
      startPage: number,
      q: string,
      providerKeys: string[],
      lng: string,
      prevByProvider: Record<string, ProviderPageInfo>,
      seq: number,
    ) => {
      prefetchAbortRef.current?.abort();
      const abort = new AbortController();
      prefetchAbortRef.current = abort;

      let page = startPage;
      let lastByProvider = prevByProvider;

      while (!abort.signal.aborted && seq === requestSeqRef.current) {
        const withMore = providerKeys.filter(
          (k) => lastByProvider[k]?.has_more,
        );
        if (withMore.length === 0) break;

        if (pageCacheRef.current.has(page)) {
          lastByProvider = pageCacheRef.current.get(page)!.byProvider;
          page++;
          continue;
        }

        await new Promise<void>((resolve) => {
          const timer = setTimeout(resolve, 300);
          abort.signal.addEventListener(
            "abort",
            () => {
              clearTimeout(timer);
              resolve();
            },
            { once: true },
          );
        });
        if (abort.signal.aborted || seq !== requestSeqRef.current) break;

        const providerPages: Record<string, number> = {};
        for (const k of withMore) providerPages[k] = page;

        try {
          const resp = await marketApi.searchMarket({
            query: q.trim(),
            provider_pages: providerPages,
            limit: PER_PROVIDER_LIMIT,
            lang: lng,
          });
          if (abort.signal.aborted || seq !== requestSeqRef.current) break;

          pageCacheRef.current.set(page, {
            results: resp.results,
            errors: resp.errors,
            byProvider: resp.by_provider,
          });
          setCacheVersion((v) => v + 1);
          lastByProvider = resp.by_provider;
          page++;
        } catch {
          break;
        }
      }
    },
    [],
  );

  const runFetch = useCallback(
    (q: string, page: number, providerKeys: string[], lng: string) => {
      const seq = ++requestSeqRef.current;
      if (!q.trim() || providerKeys.length === 0) {
        setResults([]);
        setErrors([]);
        setByProvider({});
        setCurrentPage(1);
        setLoading(false);
        return;
      }
      const providerPages: Record<string, number> = {};
      for (const k of providerKeys) providerPages[k] = page;
      setLoading(true);
      setGlobalError(null);
      marketApi
        .searchMarket({
          query: q.trim(),
          provider_pages: providerPages,
          limit: PER_PROVIDER_LIMIT,
          lang: lng,
        })
        .then((resp) => {
          if (seq !== requestSeqRef.current) return;
          applyResponse(resp, page);
          if (Object.values(resp.by_provider).some((p) => p.has_more)) {
            prefetchAllPages(
              page + 1,
              q,
              providerKeys,
              lng,
              resp.by_provider,
              seq,
            );
          }
        })
        .catch((err: unknown) => {
          if (seq !== requestSeqRef.current) return;
          setGlobalError(errorMessage(err));
          setResults([]);
          setByProvider({});
        })
        .finally(() => {
          if (seq === requestSeqRef.current) setLoading(false);
        });
    },
    [applyResponse, prefetchAllPages],
  );

  const activeQueryRef = useRef("");

  const doSearch = useCallback(
    (q: string) => {
      prefetchAbortRef.current?.abort();
      activeQueryRef.current = q;
      pageCacheRef.current.clear();
      runFetch(q, 1, providerKeyList, lang);
    },
    [providerKeyList, lang, runFetch],
  );

  const triggerSearch = useCallback(() => {
    doSearch(query);
  }, [query, doSearch]);

  const goToPage = useCallback(
    (page: number) => {
      if (sortBy !== "relevance") {
        setCurrentPage(page);
        return;
      }
      const cached = pageCacheRef.current.get(page);
      if (cached) {
        setResults(cached.results);
        setErrors(cached.errors);
        setByProvider(cached.byProvider);
        setCurrentPage(page);
        return;
      }
      runFetch(activeQueryRef.current, page, providerKeyList, lang);
    },
    [sortBy, providerKeyList, lang, runFetch],
  );

  const retry = useCallback(() => {
    if (providers.length === 0) {
      fetchProviders();
    } else {
      runFetch(activeQueryRef.current, currentPage, providerKeyList, lang);
    }
  }, [
    providers.length,
    fetchProviders,
    currentPage,
    providerKeyList,
    lang,
    runFetch,
  ]);

  const setQuery = useCallback((q: string) => {
    setQueryState(q);
  }, []);

  const lastProvLangRef = useRef("");
  useEffect(() => {
    const key = `${providerKeyList.join(",")}|${lang}`;
    if (lastProvLangRef.current === key) return;
    lastProvLangRef.current = key;
    const q = activeQueryRef.current;
    if (!q.trim()) return;
    doSearch(q);
  }, [providerKeyList, lang, doSearch]);

  const categories = useMemo(() => {
    const set = new Set<string>();
    let source: MarketResult[];
    if (sortBy !== "relevance") {
      void cacheVersion;
      source = Array.from(pageCacheRef.current.values()).flatMap(
        (c) => c.results,
      );
    } else {
      source = results;
    }
    for (const r of source) {
      const cat = r.stats?.category;
      if (typeof cat === "string" && cat) set.add(cat);
    }
    return Array.from(set).sort();
  }, [results, sortBy, cacheVersion]);

  const displayResults = useMemo(() => {
    if (sortedFilteredResults) {
      const start = (currentPage - 1) * sortedPageSize;
      return sortedFilteredResults.slice(start, start + sortedPageSize);
    }
    let list = results;
    if (filterCategory) {
      list = list.filter((r) => {
        const cat = r.stats?.category;
        return typeof cat === "string" && cat === filterCategory;
      });
    }
    return list;
  }, [
    results,
    filterCategory,
    sortedFilteredResults,
    currentPage,
    sortedPageSize,
  ]);

  const handleSetSortBy = useCallback((s: SortBy) => {
    setSortBy(s);
    setCurrentPage(1);
  }, []);

  const handleSetFilterCategory = useCallback((c: string) => {
    setFilterCategory(c);
    setCurrentPage(1);
  }, []);

  return {
    providers,
    selectedProviderKeys,
    toggleProvider,
    query,
    setQuery,
    triggerSearch,
    results,
    displayResults,
    errors,
    globalError,
    loading,
    currentPage,
    totalPages,
    totalResults,
    goToPage,
    retry,
    refreshProviders: fetchProviders,
    sortBy,
    setSortBy: handleSetSortBy,
    filterCategory,
    setFilterCategory: handleSetFilterCategory,
    categories,
  };
}
