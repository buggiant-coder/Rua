import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ChevronRight, Folder, FolderOpen, Loader2 } from "lucide-react";

import { Btn, TextInput } from "@/ui";

import styles from "./index.module.less";

export interface DirectoryPickerEntry {
  name: string;
  path: string;
}

export interface DirectoryPickerListing<T = unknown> {
  path: string;
  parent?: string | null;
  directories: DirectoryPickerEntry[];
  roots?: DirectoryPickerEntry[];
  data?: T;
}

interface DirectoryPickerProps<T> {
  value: string;
  onChange: (value: string) => void;
  browse: (path: string) => Promise<DirectoryPickerListing<T>>;
  browseRequest?: { path: string; id: number };
  createDirectory?: (
    parent: string,
    name: string,
  ) => Promise<DirectoryPickerListing<T>>;
  rootsLabel?: string;
  createLabel?: string;
  folderNameLabel?: string;
  cancelLabel?: string;
  placeholder?: string;
  disabled?: boolean;
  required?: boolean;
  id?: string;
  name?: string;
  ariaLabel?: string;
  openLabel?: string;
  closeLabel?: string;
  emptyLabel?: string;
  className?: string;
  onListingChange?: (listing: DirectoryPickerListing<T>) => void;
  onLoadingChange?: (loading: boolean) => void;
  onError?: (error: unknown) => void;
}

function splitPath(value: string): { parent: string; query: string } {
  const trimmed = value.trim();
  if (!trimmed) return { parent: "", query: "" };
  if (trimmed === "/") return { parent: "/", query: "" };
  const separator = trimmed.lastIndexOf("/");
  if (separator < 0) return { parent: ".", query: trimmed };
  return {
    parent: separator === 0 ? "/" : trimmed.slice(0, separator),
    query: trimmed.slice(separator + 1),
  };
}

export default function DirectoryPicker<T = unknown>({
  value,
  onChange,
  browse,
  browseRequest,
  createDirectory,
  rootsLabel = "Storage locations",
  createLabel = "New folder",
  folderNameLabel = "Folder name",
  cancelLabel = "Cancel",
  placeholder,
  disabled = false,
  required = false,
  id,
  name,
  ariaLabel,
  openLabel = "Browse folders",
  closeLabel = "Close folder browser",
  emptyLabel = "No folders",
  className,
  onListingChange,
  onLoadingChange,
  onError,
}: DirectoryPickerProps<T>) {
  const [open, setOpen] = useState(false);
  const [newFolderOpen, setNewFolderOpen] = useState(false);
  const [folderName, setFolderName] = useState("");
  const [creatingFolder, setCreatingFolder] = useState(false);
  const [loading, setLoading] = useState(false);
  const [listing, setListing] = useState<DirectoryPickerListing<T> | null>(
    null,
  );
  const [suggestions, setSuggestions] = useState<DirectoryPickerEntry[]>([]);
  const [suggestionsOpen, setSuggestionsOpen] = useState(false);
  const [highlightedSuggestion, setHighlightedSuggestion] = useState(-1);
  const requestIdRef = useRef(0);
  const browseRequestIdRef = useRef<number>();

  useEffect(() => {
    onLoadingChange?.(loading);
  }, [loading, onLoadingChange]);

  useEffect(
    () => () => {
      ++requestIdRef.current;
    },
    [],
  );

  const reportError = useCallback(
    (error: unknown) => {
      onError?.(error);
    },
    [onError],
  );

  const loadListing = useCallback(
    async (path: string, updateValue = true) => {
      const requestId = ++requestIdRef.current;
      setLoading(true);
      try {
        const next = await browse(path);
        if (requestId !== requestIdRef.current) return;
        setListing(next);
        onListingChange?.(next);
        if (updateValue) onChange(next.path);
      } catch (error) {
        if (requestId === requestIdRef.current) reportError(error);
      } finally {
        if (requestId === requestIdRef.current) setLoading(false);
      }
    },
    [browse, onChange, onListingChange, reportError],
  );

  useEffect(() => {
    if (!browseRequest || browseRequest.id === browseRequestIdRef.current)
      return;
    browseRequestIdRef.current = browseRequest.id;
    setOpen(true);
    void loadListing(browseRequest.path);
  }, [browseRequest, loadListing]);

  useEffect(() => {
    if (disabled) {
      setSuggestions([]);
      setSuggestionsOpen(false);
      return;
    }
    const { parent, query } = splitPath(value);
    if (!parent) {
      setSuggestions([]);
      setSuggestionsOpen(false);
      return;
    }
    let active = true;
    const timer = window.setTimeout(async () => {
      try {
        const result = await browse(parent);
        if (!active) return;
        const queryLower = query.toLocaleLowerCase();
        const matches = result.directories
          .filter((entry) =>
            entry.name.toLocaleLowerCase().startsWith(queryLower),
          )
          .slice(0, 8);
        setSuggestions(matches);
        setSuggestionsOpen(
          matches.length > 0 && matches.some((entry) => entry.path !== value),
        );
        setHighlightedSuggestion(-1);
      } catch {
        if (active) {
          setSuggestions([]);
          setSuggestionsOpen(false);
        }
      }
    }, 180);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [browse, disabled, value]);

  const chooseSuggestion = useCallback(
    (entry: DirectoryPickerEntry) => {
      onChange(entry.path);
      setSuggestionsOpen(false);
      if (open) void loadListing(entry.path, false);
    },
    [loadListing, onChange, open],
  );

  const entries = useMemo(() => listing?.directories ?? [], [listing]);

  const createFolder = async () => {
    if (
      !createDirectory ||
      !listing ||
      !folderName.trim() ||
      creatingFolder ||
      disabled ||
      loading
    )
      return;
    const requestId = ++requestIdRef.current;
    setCreatingFolder(true);
    try {
      const next = await createDirectory(listing.path, folderName.trim());
      if (requestId !== requestIdRef.current) return;
      setListing(next);
      onListingChange?.(next);
      onChange(next.path);
      setNewFolderOpen(false);
      setFolderName("");
    } catch (error) {
      if (requestId === requestIdRef.current) reportError(error);
    } finally {
      setCreatingFolder(false);
    }
  };

  return (
    <div className={[styles.root, className].filter(Boolean).join(" ")}>
      <div className={styles.pathField}>
        <div className={styles.inputWrap}>
          <TextInput
            id={id}
            name={name}
            value={value}
            onChange={(event) => onChange(event.target.value)}
            onFocus={() => setSuggestionsOpen(suggestions.length > 0)}
            onBlur={() =>
              window.setTimeout(() => setSuggestionsOpen(false), 100)
            }
            onKeyDown={(event) => {
              if (!suggestionsOpen || suggestions.length === 0) return;
              if (event.key === "ArrowDown") {
                event.preventDefault();
                setHighlightedSuggestion(
                  (current) => (current + 1) % suggestions.length,
                );
              } else if (event.key === "ArrowUp") {
                event.preventDefault();
                setHighlightedSuggestion((current) =>
                  current <= 0 ? suggestions.length - 1 : current - 1,
                );
              } else if (event.key === "Enter" && highlightedSuggestion >= 0) {
                event.preventDefault();
                chooseSuggestion(suggestions[highlightedSuggestion]);
              } else if (event.key === "Escape") {
                setSuggestionsOpen(false);
              }
            }}
            placeholder={placeholder}
            disabled={disabled || creatingFolder}
            required={required}
            aria-label={ariaLabel}
            autoComplete="off"
          />
          {suggestionsOpen && (
            <div className={styles.suggestions} role="listbox">
              {suggestions.map((entry, index) => (
                <button
                  key={entry.path}
                  type="button"
                  role="option"
                  aria-selected={highlightedSuggestion === index}
                  className={
                    highlightedSuggestion === index
                      ? styles.highlighted
                      : undefined
                  }
                  onMouseDown={(event) => event.preventDefault()}
                  onClick={() => chooseSuggestion(entry)}
                >
                  <Folder size={14} aria-hidden="true" />
                  <span>{entry.path}</span>
                </button>
              ))}
            </div>
          )}
        </div>
        <Btn
          type="button"
          variant={open ? "filled" : "outline"}
          className={styles.toggle}
          aria-label={open ? closeLabel : openLabel}
          title={open ? closeLabel : openLabel}
          disabled={disabled || creatingFolder}
          onClick={() => {
            if (!open) void loadListing(value.trim() || "~");
            setOpen((current) => !current);
          }}
        >
          {open ? <FolderOpen size={16} /> : <Folder size={16} />}
        </Btn>
      </div>

      {open && (
        <div className={styles.browser}>
          {!!listing?.roots?.length && (
            <div
              className={styles.locations}
              role="group"
              aria-label={rootsLabel}
            >
              <span>{rootsLabel}</span>
              {listing.roots.map((root) => (
                <button
                  key={root.path}
                  type="button"
                  title={root.path}
                  disabled={disabled || loading || creatingFolder}
                  onClick={() => {
                    setNewFolderOpen(false);
                    void loadListing(root.path);
                  }}
                >
                  <Folder size={14} aria-hidden="true" />
                  {root.name}
                </button>
              ))}
            </div>
          )}
          <div className={styles.browserHeader}>
            <button
              type="button"
              disabled={
                disabled || loading || creatingFolder || !listing?.parent
              }
              onClick={() =>
                listing?.parent && void loadListing(listing.parent)
              }
              aria-label="Parent folder"
            >
              ..
            </button>
            <span title={listing?.path ?? value}>{listing?.path ?? value}</span>
            {loading && (
              <Loader2
                size={14}
                className={styles.spinner}
                aria-label="Loading"
              />
            )}
          </div>
          {createDirectory && listing && (
            <div className={styles.createFolder}>
              {newFolderOpen ? (
                <>
                  <TextInput
                    value={folderName}
                    onChange={(event) => setFolderName(event.target.value)}
                    aria-label={folderNameLabel}
                    placeholder={folderNameLabel}
                    autoFocus
                    disabled={creatingFolder}
                    onKeyDown={(event) => {
                      if (event.key === "Enter") {
                        event.preventDefault();
                        void createFolder();
                      }
                    }}
                  />
                  <Btn
                    type="button"
                    size="sm"
                    disabled={
                      disabled ||
                      loading ||
                      creatingFolder ||
                      !folderName.trim()
                    }
                    onClick={() => void createFolder()}
                  >
                    {createLabel}
                  </Btn>
                  <Btn
                    type="button"
                    size="sm"
                    disabled={disabled || creatingFolder}
                    onClick={() => setNewFolderOpen(false)}
                  >
                    {cancelLabel}
                  </Btn>
                </>
              ) : (
                <Btn
                  type="button"
                  size="sm"
                  disabled={disabled || loading || creatingFolder}
                  onClick={() => setNewFolderOpen(true)}
                >
                  {createLabel}
                </Btn>
              )}
            </div>
          )}
          <div className={styles.entries}>
            {!loading && entries.length === 0 && (
              <div className={styles.empty}>{emptyLabel}</div>
            )}
            {entries.map((entry) => (
              <button
                key={entry.path}
                type="button"
                disabled={disabled || loading || creatingFolder}
                onClick={() => void loadListing(entry.path)}
              >
                <Folder size={15} aria-hidden="true" />
                <span>{entry.name}</span>
                <ChevronRight size={14} aria-hidden="true" />
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
