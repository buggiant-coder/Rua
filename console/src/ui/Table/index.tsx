import { useState, useMemo, useCallback } from "react";
import type { ReactNode, CSSProperties, HTMLAttributes } from "react";
import Loading from "../Loading";
import "./table.css";

export interface ColumnDef<T> {
  key?: string;
  title?: ReactNode;
  dataIndex?: string;
  render?: (value: any, record: T, index: number) => ReactNode;
  sorter?: boolean | ((a: T, b: T) => number);
  fixed?: "left" | "right";
  width?: number | string;
  ellipsis?: boolean;
  align?: "left" | "center" | "right";
  stackLine?: number;
  defaultSortOrder?: "ascend" | "descend" | null;
}

export interface PaginationConfig {
  current?: number;
  pageSize: number;
  total?: number;
  onChange?: (page: number, pageSize: number) => void;
}

export interface TableProps<T> {
  columns: ColumnDef<T>[];
  dataSource: T[];
  rowKey?: string | ((record: T, index: number) => string);
  loading?: boolean;
  pagination?: PaginationConfig | false;
  onRow?: (record: T, index: number) => HTMLAttributes<HTMLDivElement>;
  className?: string;
  scroll?: { x?: number | string; y?: number | string };
  emptyText?: ReactNode;
  locale?: Record<string, unknown>;
  size?: string;
  showHeader?: boolean;
  responsive?: "scroll" | "stack";
  [key: string]: unknown;
}

type SortOrder = "asc" | "desc" | null;

function getRowKey<T>(
  record: T,
  rowKey: string | ((r: T, idx: number) => string),
  index: number = 0,
): string {
  if (typeof rowKey === "function") return rowKey(record, index);
  return String((record as Record<string, unknown>)[rowKey]);
}

function colKey<T>(col: ColumnDef<T>, idx?: number): string {
  return col.key ?? col.dataIndex ?? `col-${idx ?? 0}`;
}

function getCellValue<T>(record: T, dataIndex?: string): unknown {
  if (!dataIndex) return undefined;
  return (record as Record<string, unknown>)[dataIndex];
}

function Table<T>({
  columns,
  dataSource,
  rowKey = "key",
  loading = false,
  pagination = false,
  onRow,
  className,
  scroll,
  emptyText = "No data",
  locale,
  showHeader = true,
  responsive = "scroll",
}: TableProps<T>) {
  const [sortKey, setSortKey] = useState<string | null>(null);
  const [sortOrder, setSortOrder] = useState<SortOrder>(null);

  const handleSort = useCallback(
    (col: ColumnDef<T>) => {
      if (!col.sorter) return;
      if (sortKey !== colKey(col)) {
        setSortKey(colKey(col));
        setSortOrder("asc");
      } else if (sortOrder === "asc") {
        setSortOrder("desc");
      } else {
        setSortKey(null);
        setSortOrder(null);
      }
    },
    [sortKey, sortOrder],
  );

  const sortedData = useMemo(() => {
    if (!sortKey || !sortOrder) return dataSource;
    const col = columns.find((c) => colKey(c) === sortKey);
    if (!col?.sorter) return dataSource;

    const compareFn =
      typeof col.sorter === "function"
        ? col.sorter
        : (a: T, b: T) => {
            const va = getCellValue(a, col.dataIndex);
            const vb = getCellValue(b, col.dataIndex);
            if (va == null && vb == null) return 0;
            if (va == null) return -1;
            if (vb == null) return 1;
            if (typeof va === "number" && typeof vb === "number")
              return va - vb;
            return String(va).localeCompare(String(vb));
          };

    const sorted = [...dataSource].sort(compareFn);
    return sortOrder === "desc" ? sorted.reverse() : sorted;
  }, [dataSource, columns, sortKey, sortOrder]);

  const stickyOffsets = useMemo(() => {
    const offsets = new Map<string, CSSProperties>();
    const leftCols = columns.filter((c) => c.fixed === "left");
    let leftAcc = 0;
    for (const col of leftCols) {
      offsets.set(colKey(col), {
        position: "sticky",
        left: leftAcc,
        zIndex: 2,
      });
      leftAcc += typeof col.width === "number" ? col.width : 0;
    }
    const rightCols = columns.filter((c) => c.fixed === "right").reverse();
    let rightAcc = 0;
    for (const col of rightCols) {
      offsets.set(colKey(col), {
        position: "sticky",
        right: rightAcc,
        zIndex: 2,
      });
      rightAcc += typeof col.width === "number" ? col.width : 0;
    }
    return offsets;
  }, [columns]);

  const wrapperStyle: CSSProperties = {};
  if (scroll?.x) wrapperStyle.overflowX = "auto";
  if (scroll?.y) {
    wrapperStyle.overflowY = "auto";
    wrapperStyle.maxHeight = scroll.y;
  }

  const tableStyle: CSSProperties = {};
  if (scroll?.x) tableStyle.minWidth = scroll.x;
  const gridTemplateColumns = columns
    .map((col) => {
      if (typeof col.width === "number") return `${col.width}px`;
      if (typeof col.width === "string") return col.width;
      return "minmax(0, 1fr)";
    })
    .join(" ");
  const hasStackLines = columns.some((col) => col.stackLine != null);
  const resolvedEmptyText =
    (locale?.emptyText as ReactNode | undefined) ?? emptyText;

  const cls = [
    "rua-ui-table-wrapper",
    responsive === "stack" && "rua-ui-table-wrapper--stack",
    className,
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <Loading spinning={loading}>
      <div className={cls}>
        <div className="rua-ui-table-scroll" style={wrapperStyle}>
          <div
            className="rua-ui-table"
            style={
              {
                ...tableStyle,
                "--rua-ui-table-columns": gridTemplateColumns,
              } as CSSProperties
            }
            role="table"
          >
            {showHeader && (
              <div
                className="rua-ui-table-row rua-ui-table-header"
                role="row"
              >
                {columns.map((col, ci) => {
                  const ck = colKey(col, ci);
                  const sortable = !!col.sorter;
                  const active = sortKey === ck;
                  const thStyle: CSSProperties = {
                    ...stickyOffsets.get(ck),
                    textAlign: col.align,
                    justifyContent:
                      col.align === "right"
                        ? "flex-end"
                        : col.align === "center"
                        ? "center"
                        : undefined,
                  };
                  return (
                    <div
                      key={ck}
                      className={[
                        "rua-ui-table-th",
                        sortable && "rua-ui-table-th--sortable",
                        active && `rua-ui-table-th--${sortOrder}`,
                        col.fixed && "rua-ui-table-cell--fixed",
                      ]
                        .filter(Boolean)
                        .join(" ")}
                      style={thStyle}
                      onClick={sortable ? () => handleSort(col) : undefined}
                      role="columnheader"
                    >
                      <span className="rua-ui-table-th-content">
                        {col.title}
                        {sortable && (
                          <span className="rua-ui-table-sorter">
                            <span
                              className={`rua-ui-table-sorter-up${
                                active && sortOrder === "asc" ? " active" : ""
                              }`}
                            />
                            <span
                              className={`rua-ui-table-sorter-down${
                                active && sortOrder === "desc" ? " active" : ""
                              }`}
                            />
                          </span>
                        )}
                      </span>
                    </div>
                  );
                })}
              </div>
            )}
            <div role="rowgroup">
              {sortedData.length === 0 ? (
                <div className="rua-ui-table-empty">{resolvedEmptyText}</div>
              ) : (
                sortedData.map((record, idx) => {
                  const key = getRowKey(record, rowKey, idx);
                  const rowProps = onRow?.(record, idx) ?? {};
                  const rowClassName = [
                    "rua-ui-table-row",
                    rowProps.className,
                  ]
                    .filter(Boolean)
                    .join(" ");
                  const renderCell = (col: ColumnDef<T>, ci: number) => {
                    const ck = colKey(col, ci);
                    const val = getCellValue(record, col.dataIndex);
                    const tdStyle: CSSProperties = {
                      ...stickyOffsets.get(ck),
                      textAlign: col.align,
                      justifyContent:
                        col.align === "right"
                          ? "flex-end"
                          : col.align === "center"
                          ? "center"
                          : undefined,
                    };
                    return (
                      <div
                        key={ck}
                        className={[
                          "rua-ui-table-td",
                          col.ellipsis && "rua-ui-table-td--ellipsis",
                          col.fixed && "rua-ui-table-cell--fixed",
                        ]
                          .filter(Boolean)
                          .join(" ")}
                        style={tdStyle}
                        role="cell"
                      >
                        {col.render
                          ? col.render(val, record, idx)
                          : (val as ReactNode)}
                      </div>
                    );
                  };

                  const stackLineGroups = hasStackLines
                    ? columns.reduce((groups, col, ci) => {
                        const line = col.stackLine ?? ci + 1;
                        if (!groups.has(line)) groups.set(line, []);
                        groups.get(line)!.push({ col, ci });
                        return groups;
                      }, new Map<number, Array<{ col: ColumnDef<T>; ci: number }>>())
                    : null;

                  return (
                    <div
                      key={key}
                      {...rowProps}
                      className={rowClassName}
                      role="row"
                    >
                      {stackLineGroups
                        ? [...stackLineGroups.entries()]
                            .sort(([a], [b]) => a - b)
                            .map(([line, items]) => (
                              <div
                                key={line}
                                className="rua-ui-table-stack-line"
                              >
                                {items.map(({ col, ci }) =>
                                  renderCell(col, ci),
                                )}
                              </div>
                            ))
                        : columns.map(renderCell)}
                    </div>
                  );
                })
              )}
            </div>
          </div>
        </div>

        {pagination && (
          <div className="rua-ui-table-pagination">
            <TablePagination {...pagination} />
          </div>
        )}
      </div>
    </Loading>
  );
}

function TablePagination({
  current = 1,
  pageSize,
  total = 0,
  onChange,
}: PaginationConfig) {
  const totalPages = Math.max(1, Math.ceil(total / pageSize));

  const pages = useMemo(() => {
    const result: (number | "...")[] = [];
    for (let i = 1; i <= totalPages; i++) {
      if (
        i === 1 ||
        i === totalPages ||
        (i >= current - 1 && i <= current + 1)
      ) {
        result.push(i);
      } else if (result[result.length - 1] !== "...") {
        result.push("...");
      }
    }
    return result;
  }, [current, totalPages]);

  return (
    <nav className="rua-ui-table-pager" aria-label="Pagination">
      <button
        className="rua-ui-table-pager-btn"
        disabled={current <= 1}
        onClick={() => onChange?.(current - 1, pageSize)}
      >
        ‹
      </button>
      {pages.map((p, i) =>
        p === "..." ? (
          <span key={`e${i}`} className="rua-ui-table-pager-ellipsis">
            …
          </span>
        ) : (
          <button
            key={p}
            className={`rua-ui-table-pager-btn${
              p === current ? " active" : ""
            }`}
            onClick={() => onChange?.(p as number, pageSize)}
          >
            {p}
          </button>
        ),
      )}
      <button
        className="rua-ui-table-pager-btn"
        disabled={current >= totalPages}
        onClick={() => onChange?.(current + 1, pageSize)}
      >
        ›
      </button>
    </nav>
  );
}

export default Table;
