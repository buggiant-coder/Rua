import "./pagination.css";

interface PaginationProps {
  current: number;
  total: number;
  pageSize: number;
  onChange: (page: number) => void;
  className?: string;
  showSizeChanger?: boolean;
  size?: string;
}

export default function Pagination({
  current,
  total,
  pageSize,
  onChange,
  className,
}: PaginationProps) {
  const totalPages = Math.max(1, Math.ceil(total / pageSize));
  const pages = buildPages(current, totalPages);

  const cls = ["rua-ui-pagination", className].filter(Boolean).join(" ");

  return (
    <nav className={cls}>
      <button
        className="rua-ui-pagination-btn"
        disabled={current <= 1}
        onClick={() => onChange(current - 1)}
      >
        ‹
      </button>
      {pages.map((p, i) =>
        p === "..." ? (
          <span key={`e${i}`} className="rua-ui-pagination-ellipsis">
            …
          </span>
        ) : (
          <button
            key={p}
            className={`rua-ui-pagination-btn${
              p === current ? " rua-ui-pagination-btn--active" : ""
            }`}
            onClick={() => onChange(p as number)}
          >
            {p}
          </button>
        ),
      )}
      <button
        className="rua-ui-pagination-btn"
        disabled={current >= totalPages}
        onClick={() => onChange(current + 1)}
      >
        ›
      </button>
    </nav>
  );
}

function buildPages(current: number, total: number): (number | "...")[] {
  if (total <= 7) return Array.from({ length: total }, (_, i) => i + 1);
  const pages: (number | "...")[] = [1];
  const left = Math.max(2, current - 1);
  const right = Math.min(total - 1, current + 1);
  if (left > 2) pages.push("...");
  for (let i = left; i <= right; i++) pages.push(i);
  if (right < total - 1) pages.push("...");
  pages.push(total);
  return pages;
}
