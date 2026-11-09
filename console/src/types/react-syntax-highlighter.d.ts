declare module "react-syntax-highlighter" {
  import type {
    ComponentType,
    CSSProperties,
    ElementType,
    ReactNode,
  } from "react";

  export type SyntaxHighlighterProps = {
    children?: ReactNode;
    className?: string;
    language?: string;
    PreTag?: ElementType;
    style?: Record<string, CSSProperties>;
    customStyle?: CSSProperties;
    codeTagProps?: Record<string, unknown>;
  };

  export const Prism: ComponentType<SyntaxHighlighterProps>;
}
