import type { ProjectGitCommit } from "@/api/types/project";

const COLORS = [
  "#3794ff",
  "#e2b93d",
  "#b180d7",
  "#4ec9b0",
  "#f48771",
  "#75beff",
];

export type GitGraphLane = { oid: string; color: string };

export type GitGraphRow = {
  commit: ProjectGitCommit;
  lane: number;
  startsLane: boolean;
  incoming: GitGraphLane[];
  outgoing: GitGraphLane[];
};

export function layoutGitGraph(commits: ProjectGitCommit[]): GitGraphRow[] {
  let lanes: GitGraphLane[] = [];
  let nextColor = 0;

  return commits.map((commit) => {
    let lane = lanes.findIndex(({ oid }) => oid === commit.oid);
    const startsLane = lane < 0;
    if (lane < 0) {
      lane = lanes.length;
      lanes.push({
        oid: commit.oid,
        color: COLORS[nextColor++ % COLORS.length],
      });
    }

    const incoming = lanes.map((item) => ({ ...item }));
    const current = lanes[lane];
    const outgoing = lanes.filter(({ oid }) => oid !== commit.oid);

    commit.parent_oids.forEach((parentOid, parentIndex) => {
      if (outgoing.some(({ oid }) => oid === parentOid)) return;
      const color =
        parentIndex === 0 ? current.color : COLORS[nextColor++ % COLORS.length];
      outgoing.splice(Math.min(lane + parentIndex, outgoing.length), 0, {
        oid: parentOid,
        color,
      });
    });

    lanes = outgoing;
    return {
      commit,
      lane,
      startsLane,
      incoming,
      outgoing: outgoing.map((item) => ({ ...item })),
    };
  });
}
