import { useMemo } from "react";

import type { ProjectGitCommit } from "@/api/types/project";

import styles from "./index.module.less";
import { layoutGitGraph, type GitGraphRow } from "./gitHistoryLayout";

const LANE_WIDTH = 14;
const ROW_HEIGHT = 38;
const GRAPH_PADDING = 10;
function laneX(index: number) {
  return GRAPH_PADDING + index * LANE_WIDTH;
}

function curve(fromX: number, fromY: number, toX: number, toY: number) {
  const middle = (fromY + toY) / 2;
  return `M ${fromX} ${fromY} C ${fromX} ${middle}, ${toX} ${middle}, ${toX} ${toY}`;
}

function GraphCell({ row, width }: { row: GitGraphRow; width: number }) {
  const nodeX = laneX(row.lane);
  const middleY = ROW_HEIGHT / 2;
  const current = row.incoming[row.lane];

  return (
    <svg
      className={styles.gitGraphCell}
      width={width}
      height={ROW_HEIGHT}
      viewBox={`0 0 ${width} ${ROW_HEIGHT}`}
      aria-hidden="true"
    >
      {row.incoming.map((input, inputIndex) => {
        if (inputIndex === row.lane) {
          if (row.startsLane) return null;
          return (
            <path
              key={`in:${input.oid}`}
              d={`M ${nodeX} 0 L ${nodeX} ${middleY}`}
              stroke={input.color}
            />
          );
        }
        const outputIndex = row.outgoing.findIndex(
          ({ oid }) => oid === input.oid,
        );
        if (outputIndex < 0) return null;
        return (
          <path
            key={`through:${input.oid}`}
            d={curve(laneX(inputIndex), 0, laneX(outputIndex), ROW_HEIGHT)}
            stroke={input.color}
          />
        );
      })}
      {row.commit.parent_oids.map((parentOid) => {
        const outputIndex = row.outgoing.findIndex(
          ({ oid }) => oid === parentOid,
        );
        if (outputIndex < 0) return null;
        const parent = row.outgoing[outputIndex];
        const color =
          parentOid === row.commit.parent_oids[0]
            ? current.color
            : parent.color;
        return (
          <path
            key={`out:${parentOid}`}
            d={curve(nodeX, middleY, laneX(outputIndex), ROW_HEIGHT)}
            stroke={color}
          />
        );
      })}
      <circle
        cx={nodeX}
        cy={middleY}
        r="4"
        fill="var(--surface)"
        stroke={current.color}
      />
    </svg>
  );
}

function formatRef(reference: string) {
  return reference
    .replace(/^HEAD -> /, "")
    .replace(/^tag: /, "")
    .replace(/^refs\/heads\//, "")
    .replace(/^refs\/remotes\//, "");
}

export function GitHistoryGraph({ commits }: { commits: ProjectGitCommit[] }) {
  const rows = useMemo(() => layoutGitGraph(commits), [commits]);
  const laneCount = Math.max(
    1,
    ...rows.flatMap((row) => [row.incoming.length, row.outgoing.length]),
  );
  const graphWidth = GRAPH_PADDING * 2 + (laneCount - 1) * LANE_WIDTH;

  return rows.map((row) => (
    <div className={styles.gitCommitRow} key={row.commit.oid}>
      <GraphCell row={row} width={graphWidth} />
      <div className={styles.gitCommitContent} title={row.commit.summary}>
        <div className={styles.gitCommitSummary}>{row.commit.summary}</div>
        <div className={styles.gitCommitMeta}>
          <code>{row.commit.oid.slice(0, 8)}</code>
          {row.commit.refs.map((reference) => (
            <span className={styles.gitRefLabel} key={reference}>
              {formatRef(reference)}
            </span>
          ))}
        </div>
      </div>
    </div>
  ));
}
