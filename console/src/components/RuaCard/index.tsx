import type { CSSProperties, MouseEvent, ReactNode } from "react";
import styles from "./index.module.less";

export interface RuaCardProps {
  icon?: ReactNode;

  title?: ReactNode;

  tags?: ReactNode[];

  actions?: ReactNode[];

  hoverable?: boolean;

  onClick?: (e: MouseEvent<HTMLDivElement>) => void;

  selected?: boolean;

  disabled?: boolean;

  onMouseEnter?: () => void;
  onMouseLeave?: () => void;

  className?: string;

  style?: CSSProperties;

  children?: ReactNode;

  footer?: ReactNode;

  bare?: boolean;
}

export function RuaCard({
  icon,
  title,
  tags,
  actions,
  hoverable,
  onClick,
  selected,
  disabled,
  onMouseEnter,
  onMouseLeave,
  className,
  style,
  children,
  footer,
  bare,
}: RuaCardProps) {
  const rootClass = [
    styles.card,
    bare && styles.bare,
    hoverable && styles.hoverable,
    selected && styles.selected,
    disabled && styles.disabled,
    className,
  ]
    .filter(Boolean)
    .join(" ");

  const handleClick = (e: MouseEvent<HTMLDivElement>) => {
    if (!hoverable || disabled) return;
    onClick?.(e);
  };

  const hasHeader = !!(
    title ||
    icon ||
    (tags && tags.length > 0) ||
    (actions && actions.length > 0)
  );

  const actionCount = actions?.length ?? 0;
  const headerPaddingRight =
    actionCount === 0 ? 0 : 12 + actionCount * 32 + (actionCount - 1) * 8 + 12;

  return (
    <div
      className={rootClass}
      style={style}
      onClick={hoverable && !disabled ? handleClick : undefined}
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
    >
      {hasHeader && (
        <div
          className={styles.header}
          style={{ paddingRight: headerPaddingRight }}
        >
          {icon ? <div className={styles.icon}>{icon}</div> : null}
          <div className={styles.titleWrap}>
            {title ? <span className={styles.title}>{title}</span> : null}
            {tags && tags.length > 0 && (
              <div className={styles.tags}>
                {tags.map((t, i) => (
                  <span key={i}>{t}</span>
                ))}
              </div>
            )}
          </div>
          {actions && actions.length > 0 && (
            <div
              className={styles.actions}
              onClick={(e) => e.stopPropagation()}
            >
              {actions.map((a, i) => (
                <span key={i}>{a}</span>
              ))}
            </div>
          )}
        </div>
      )}
      {children ? <div className={styles.body}>{children}</div> : null}
      {footer ? (
        <>
          <hr className="rua-divider" style={{ margin: 0 }} />
          <div className={styles.footer} onClick={(e) => e.stopPropagation()}>
            {footer}
          </div>
        </>
      ) : null}
    </div>
  );
}

export default RuaCard;
