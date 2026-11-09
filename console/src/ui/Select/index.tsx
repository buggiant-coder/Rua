import {
  useState,
  useRef,
  useMemo,
  useCallback,
  Children,
  isValidElement,
  type ReactNode,
} from "react";
import {
  useFloating,
  useClick,
  useDismiss,
  useInteractions,
  useTransitionStyles,
  autoUpdate,
  offset,
  flip,
  shift,
  size,
  FloatingPortal,
} from "@floating-ui/react";
import { ChevronDown } from "lucide-react";
import Tag from "../Tag";
import "./select.css";

export interface SelectOption {
  value: string | number;
  label: ReactNode;
  disabled?: boolean;
}

type SelectChangeValue = string | string[] | undefined;

type SelectValue = string | string[] | number | undefined;

export interface SelectProps<TValue extends SelectValue = string> {
  value?: TValue;
  onChange?: (value: TValue) => void;
  options?: SelectOption[];
  placeholder?: string;
  disabled?: boolean;
  mode?: "multiple" | "tags";
  showSearch?: boolean;
  allowCustomValue?: boolean;
  filterOption?: (input: string, option: SelectOption) => boolean;
  allowClear?: boolean;
  className?: string;
  style?: React.CSSProperties;
  loading?: boolean;
  children?: ReactNode;
  size?: "sm" | "md" | "lg" | "small" | "middle" | "large";
  tokenSeparators?: string[];
  maxCount?: number;
  optionFilterProp?: string;
  optionLabelProp?: string;
  popupClassName?: string;
  onDropdownVisibleChange?: (open: boolean) => void;
  suffixIcon?: React.ReactNode;
  dropdownRender?: (menu: React.ReactNode) => React.ReactNode;
  searchValue?: string;
  onSearch?: (value: string) => void;
  [key: string]: unknown;
}

const defaultFilter: SelectProps["filterOption"] = (input, option) => {
  const label =
    typeof option.label === "string" ? option.label : String(option.value);
  return label.toLowerCase().includes(input.toLowerCase());
};

function Option(props: {
  value: string | number;
  children?: ReactNode;
  disabled?: boolean;
  key?: string | number;
  label?: ReactNode;
}) {
  void props;
  return null;
}

function Select<TValue extends SelectValue = string>({
  value,
  onChange,
  options: optionsProp,
  placeholder,
  disabled,
  mode,
  showSearch,
  allowCustomValue,
  filterOption = defaultFilter,
  allowClear,
  className,
  style,
  loading,
  children,
  popupClassName,
  searchValue,
  onSearch,
  suffixIcon,
  ...rest
}: SelectProps<TValue>) {
  const options: SelectOption[] = useMemo(() => {
    if (optionsProp && optionsProp.length > 0) return optionsProp;
    if (!children) return [];
    const result: SelectOption[] = [];
    Children.forEach(children, (child) => {
      if (
        isValidElement<{
          value: string | number;
          children?: ReactNode;
          disabled?: boolean;
          label?: ReactNode;
        }>(child)
      ) {
        result.push({
          value: String(child.props.value),
          label: child.props.children ?? child.props.label ?? child.props.value,
          disabled: child.props.disabled,
        });
      }
    });
    return result;
  }, [optionsProp, children]);
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const isSearchControlled = searchValue !== undefined;
  const currentSearch = isSearchControlled ? searchValue : search;
  const inputRef = useRef<HTMLInputElement>(null);

  const isMultiple = mode === "multiple" || mode === "tags";
  const selectedValues: string[] = useMemo(
    () =>
      isMultiple
        ? Array.isArray(value)
          ? value.map(String)
          : value != null
          ? [String(value)]
          : []
        : [],
    [isMultiple, value],
  );
  const singleValue = isMultiple
    ? undefined
    : value != null
    ? String(value)
    : undefined;

  const { refs, floatingStyles, context } = useFloating({
    open,
    onOpenChange(next) {
      if (disabled) return;
      setOpen(next);
      if (!next && !isSearchControlled) setSearch("");
    },
    placement: "bottom-start",
    middleware: [
      offset(4),
      flip(),
      shift({ padding: 8 }),
      size({
        apply({ rects, elements }) {
          Object.assign(elements.floating.style, {
            minWidth: `${rects.reference.width}px`,
          });
        },
      }),
    ],
    whileElementsMounted: autoUpdate,
  });

  const click = useClick(context, { toggle: true });
  const dismiss = useDismiss(context);
  const { getReferenceProps, getFloatingProps } = useInteractions([
    click,
    dismiss,
  ]);
  const { isMounted, styles: transitionStyles } = useTransitionStyles(context, {
    duration: 150,
    initial: { opacity: 0 },
  });

  const updateSearch = useCallback(
    (next: string) => {
      if (!isSearchControlled) setSearch(next);
      onSearch?.(next);
    },
    [isSearchControlled, onSearch],
  );

  const emitChange = useCallback(
    (next: SelectChangeValue) => {
      onChange?.(next as TValue);
    },
    [onChange],
  );

  const filtered = useMemo(() => {
    if (!currentSearch) return options;
    return options.filter((o) => filterOption!(currentSearch, o));
  }, [options, currentSearch, filterOption]);

  const handleSelect = useCallback(
    (optValue: string) => {
      if (isMultiple) {
        const next = selectedValues.includes(optValue)
          ? selectedValues.filter((v) => v !== optValue)
          : [...selectedValues, optValue];
        emitChange(next);
      } else {
        emitChange(optValue);
        setOpen(false);
        if (isSearchControlled) {
          onSearch?.(optValue);
        } else {
          setSearch("");
        }
      }
    },
    [isMultiple, selectedValues, emitChange, isSearchControlled, onSearch],
  );

  const handleRemoveTag = useCallback(
    (val: string) => {
      emitChange(selectedValues.filter((v) => v !== val));
    },
    [selectedValues, emitChange],
  );

  const handleClear = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      if (isMultiple) {
        emitChange([]);
      } else {
        emitChange(undefined);
      }
      updateSearch("");
    },
    [isMultiple, emitChange, updateSearch],
  );

  const singleLabel = useMemo(() => {
    if (singleValue == null) return undefined;
    const opt = options.find((o) => o.value === singleValue);
    return opt ? opt.label : singleValue;
  }, [singleValue, options]);

  const showClear =
    allowClear &&
    !disabled &&
    (isMultiple ? selectedValues.length > 0 : singleValue != null);

  const cls = [
    "rua-ui-select",
    disabled && "rua-ui-select--disabled",
    open && "rua-ui-select--open",
    className,
  ]
    .filter(Boolean)
    .join(" ");
  const dropdownCls = [
    "rua-ui-select-dropdown",
    filtered.length === 1 && "rua-ui-select-dropdown--single",
    popupClassName,
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <>
      <div
        ref={refs.setReference}
        role="combobox"
        aria-expanded={open}
        aria-disabled={disabled || undefined}
        className={cls}
        style={style}
        {...rest}
        {...getReferenceProps()}
      >
        <div className="rua-ui-select-selector">
          {isMultiple && (
            <div className="rua-ui-select-tags">
              {selectedValues.map((v) => {
                const opt = options.find((o) => o.value === v);
                return (
                  <Tag
                    key={v}
                    closable={!disabled}
                    onClose={() => handleRemoveTag(v)}
                  >
                    {opt ? opt.label : v}
                  </Tag>
                );
              })}
            </div>
          )}
          {showSearch || isMultiple ? (
            <input
              ref={inputRef}
              className="rua-ui-select-search"
              value={currentSearch}
              onChange={(e) => {
                const next = e.target.value;
                updateSearch(next);
                if (allowCustomValue && !isMultiple) {
                  emitChange(next);
                }
                if (!open) setOpen(true);
              }}
              placeholder={
                isMultiple && selectedValues.length > 0
                  ? undefined
                  : singleLabel != null || currentSearch
                  ? undefined
                  : placeholder
              }
              disabled={disabled}
              onFocus={() => {
                if (!open) setOpen(true);
              }}
              onClick={(e) => e.stopPropagation()}
              onKeyDown={(e) => {
                if (
                  e.key === "Backspace" &&
                  !currentSearch &&
                  isMultiple &&
                  selectedValues.length > 0
                ) {
                  handleRemoveTag(selectedValues[selectedValues.length - 1]);
                }
              }}
            />
          ) : null}
          {!showSearch &&
            !isMultiple &&
            singleLabel != null &&
            !currentSearch && (
              <span className="rua-ui-select-value">{singleLabel}</span>
            )}
          {!showSearch &&
            !isMultiple &&
            singleLabel == null &&
            !currentSearch &&
            placeholder && (
              <span className="rua-ui-select-placeholder">{placeholder}</span>
            )}
          {showSearch &&
            !isMultiple &&
            singleLabel != null &&
            !currentSearch && (
              <span className="rua-ui-select-value">{singleLabel}</span>
            )}
        </div>
        <div className="rua-ui-select-suffix">
          {loading && <span className="rua-ui-select-loading" />}
          {showClear && (
            <span
              className="rua-ui-select-clear"
              onClick={handleClear}
              role="button"
            >
              &times;
            </span>
          )}
          {suffixIcon ?? (
            <ChevronDown
              className={`rua-ui-select-arrow${
                open ? " rua-ui-select-arrow--open" : ""
              }`}
              aria-hidden="true"
            />
          )}
        </div>
      </div>
      {isMounted && (
        <FloatingPortal>
          <div
            ref={refs.setFloating}
            style={{ ...floatingStyles, ...transitionStyles }}
            className={dropdownCls}
            {...getFloatingProps()}
          >
            {filtered.length === 0 ? (
              <div className="rua-ui-select-empty">No options</div>
            ) : (
              <div className="rua-ui-select-options">
                {filtered.map((opt) => {
                  const optStr = String(opt.value);
                  const selected = isMultiple
                    ? selectedValues.includes(optStr)
                    : optStr === singleValue;
                  const itemCls = [
                    "rua-ui-select-option",
                    selected && "rua-ui-select-option--selected",
                    opt.disabled && "rua-ui-select-option--disabled",
                  ]
                    .filter(Boolean)
                    .join(" ");
                  return (
                    <div
                      key={optStr}
                      role="option"
                      aria-selected={selected}
                      aria-disabled={opt.disabled || undefined}
                      className={itemCls}
                      onClick={
                        opt.disabled ? undefined : () => handleSelect(optStr)
                      }
                    >
                      {isMultiple && (
                        <span className="rua-ui-select-option-check">
                          {selected ? "✓" : ""}
                        </span>
                      )}
                      <span className="rua-ui-select-option-label">
                        {opt.label}
                      </span>
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        </FloatingPortal>
      )}
    </>
  );
}

Select.Option = Option;
export default Select;
