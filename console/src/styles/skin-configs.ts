export type SkinName = "industrial" | "standard" | "cute" | "pill";
export type ColorSchemeName =
  | "red"
  | "orange"
  | "yellow"
  | "green"
  | "cyan"
  | "blue"
  | "purple"
  | "mono";

export interface SkinShapeTokens {
  borderRadius: number;
  borderRadiusSM: number;
  borderRadiusLG: number;
  borderRadiusXS: number;
}

export interface SkinMeta {
  nameKey: string;
  descKey: string;
  preview: string;
}

export const skinConfigs: Record<SkinName, SkinShapeTokens> = {
  industrial: {
    borderRadius: 0,
    borderRadiusSM: 0,
    borderRadiusLG: 0,
    borderRadiusXS: 0,
  },
  standard: {
    borderRadius: 6,
    borderRadiusSM: 2,
    borderRadiusLG: 12,
    borderRadiusXS: 2,
  },
  cute: {
    borderRadius: 12,
    borderRadiusSM: 8,
    borderRadiusLG: 16,
    borderRadiusXS: 6,
  },
  pill: {
    borderRadius: 9999,
    borderRadiusSM: 9999,
    borderRadiusLG: 9999,
    borderRadiusXS: 9999,
  },
};

export const skinMeta: Record<SkinName, SkinMeta> = {
  industrial: {
    nameKey: "appearance.skinIndustrial",
    descKey: "appearance.skinIndustrialDesc",
    preview: "#868e96",
  },
  standard: {
    nameKey: "appearance.skinStandard",
    descKey: "appearance.skinStandardDesc",
    preview: "#228be6",
  },
  cute: {
    nameKey: "appearance.skinCute",
    descKey: "appearance.skinCuteDesc",
    preview: "#f06595",
  },
  pill: {
    nameKey: "appearance.skinPill",
    descKey: "appearance.skinPillDesc",
    preview: "#be4bdb",
  },
};

export const SKIN_NAMES: SkinName[] = [
  "industrial",
  "standard",
  "cute",
  "pill",
];

export interface ColorSchemeMeta {
  nameKey: string;
  descKey: string;
  preview: {
    primary: string;
    surface: string;
    accent: string;
  };
}

export const colorSchemeMeta: Record<ColorSchemeName, ColorSchemeMeta> = {
  red: {
    nameKey: "appearance.colorRed",
    descKey: "appearance.colorRedDesc",
    preview: {
      primary: "#f03e3e",
      surface: "#ffffff",
      accent: "#ff8787",
    },
  },
  orange: {
    nameKey: "appearance.colorOrange",
    descKey: "appearance.colorOrangeDesc",
    preview: {
      primary: "#f76707",
      surface: "#ffffff",
      accent: "#ff922b",
    },
  },
  yellow: {
    nameKey: "appearance.colorYellow",
    descKey: "appearance.colorYellowDesc",
    preview: {
      primary: "#ffd43b",
      surface: "#ffffff",
      accent: "#fff3bf",
    },
  },
  green: {
    nameKey: "appearance.colorGreen",
    descKey: "appearance.colorGreenDesc",
    preview: {
      primary: "#37b24d",
      surface: "#ffffff",
      accent: "#69db7c",
    },
  },
  cyan: {
    nameKey: "appearance.colorCyan",
    descKey: "appearance.colorCyanDesc",
    preview: {
      primary: "#15aabf",
      surface: "#ffffff",
      accent: "#66d9e8",
    },
  },
  blue: {
    nameKey: "appearance.colorBlue",
    descKey: "appearance.colorBlueDesc",
    preview: {
      primary: "#228be6",
      surface: "#ffffff",
      accent: "#4dabf7",
    },
  },
  purple: {
    nameKey: "appearance.colorPurple",
    descKey: "appearance.colorPurpleDesc",
    preview: {
      primary: "#7950f2",
      surface: "#ffffff",
      accent: "#b197fc",
    },
  },
  mono: {
    nameKey: "appearance.colorMono",
    descKey: "appearance.colorMonoDesc",
    preview: {
      primary: "#111111",
      surface: "#ffffff",
      accent: "#a1a1aa",
    },
  },
};

export const COLOR_SCHEME_NAMES: ColorSchemeName[] = [
  "red",
  "orange",
  "yellow",
  "green",
  "cyan",
  "blue",
  "purple",
  "mono",
];
