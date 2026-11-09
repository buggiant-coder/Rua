import type { ReactNode } from "react";
import {
  Activity,
  ArrowLeftRight,
  BarChart3,
  Blocks,
  Bot,
  Brain,
  Bug,
  Calendar,
  Cpu,
  FileText,
  FolderKanban,
  Globe,
  GitBranch,
  HardDrive,
  MessageSquare,
  MessagesSquare,
  Mic,
  Package,
  Paintbrush,
  Plug,
  RadioTower,
  RefreshCcw,
  Info,
  Monitor,
  Shield,
  SlidersHorizontal,
  SquareTerminal,
  Wrench,
  Zap,
} from "lucide-react";
export interface NavItem {
  key: string;
  path: string | null;
  labelKey: string;
  icon: ReactNode;
}

export interface NavGroup {
  key: string;
  labelKey: string | null;
  items: NavItem[];
}

export const NAV_GROUPS: NavGroup[] = [
  {
    key: "primary",
    labelKey: null,
    items: [
      {
        key: "chat",
        path: null,
        labelKey: "nav.chat",
        icon: <MessageSquare size={18} />,
      },
      {
        key: "projects",
        path: "/projects",
        labelKey: "nav.projects",
        icon: <FolderKanban size={18} />,
      },
      {
        key: "terminal",
        path: "/terminal",
        labelKey: "nav.terminal",
        icon: <SquareTerminal size={18} />,
      },
    ],
  },
  {
    key: "control",
    labelKey: "nav.control",
    items: [
      {
        key: "sessions",
        path: "/sessions",
        labelKey: "nav.sessions",
        icon: <MessagesSquare size={18} />,
      },
      {
        key: "channels",
        path: "/channels",
        labelKey: "nav.channels",
        icon: <RadioTower size={18} />,
      },
      {
        key: "cron-jobs",
        path: "/cron-jobs",
        labelKey: "nav.cronJobs",
        icon: <Calendar size={18} />,
      },
      {
        key: "heartbeat",
        path: "/heartbeat",
        labelKey: "nav.heartbeat",
        icon: <Activity size={18} />,
      },
    ],
  },
  {
    key: "agent",
    labelKey: "nav.agent",
    items: [
      {
        key: "prompts",
        path: "/prompts",
        labelKey: "nav.workspace",
        icon: <FileText size={18} />,
      },
      {
        key: "skills",
        path: "/skills",
        labelKey: "nav.skills",
        icon: <Blocks size={18} />,
      },
      {
        key: "mcp",
        path: "/mcp",
        labelKey: "nav.mcp",
        icon: <Plug size={22} />,
      },
      {
        key: "acp",
        path: "/acp",
        labelKey: "nav.acp",
        icon: <Zap size={18} />,
      },
      {
        key: "tools",
        path: "/tools",
        labelKey: "nav.tools",
        icon: <Wrench size={18} />,
      },
      {
        key: "desktop-tools",
        path: "/desktop-tools",
        labelKey: "nav.desktopTools",
        icon: <Monitor size={18} />,
      },
      {
        key: "hooks",
        path: "/hooks",
        labelKey: "nav.hooks",
        icon: <GitBranch size={18} />,
      },
      {
        key: "memory",
        path: "/memory",
        labelKey: "nav.memory",
        icon: <Brain size={18} />,
      },
      {
        key: "storage",
        path: "/storage",
        labelKey: "nav.storage",
        icon: <HardDrive size={18} />,
      },
    ],
  },
  {
    key: "multi-device",
    labelKey: "nav.multiDevice",
    items: [
      {
        key: "device-sync",
        path: "/settings/device-sync",
        labelKey: "nav.deviceSync",
        icon: <RefreshCcw size={18} />,
      },
      {
        key: "airdrop-transfer",
        path: "/settings/airdrop-transfer",
        labelKey: "nav.airdropTransfer",
        icon: <ArrowLeftRight size={18} />,
      },
    ],
  },
  {
    key: "settings",
    labelKey: "nav.settings",
    items: [
      {
        key: "agents",
        path: "/agents",
        labelKey: "nav.agents",
        icon: <Bot size={18} />,
      },
      {
        key: "models",
        path: "/models",
        labelKey: "nav.models",
        icon: <Cpu size={18} />,
      },
      {
        key: "agent-config",
        path: "/agent-config",
        labelKey: "nav.agentConfig",
        icon: <SlidersHorizontal size={18} />,
      },
      {
        key: "security",
        path: "/security",
        labelKey: "nav.security",
        icon: <Shield size={18} />,
      },
      {
        key: "token-usage",
        path: "/token-usage",
        labelKey: "nav.tokenUsage",
        icon: <BarChart3 size={18} />,
      },
      {
        key: "environments",
        path: "/environments",
        labelKey: "nav.environments",
        icon: <Globe size={18} />,
      },
      {
        key: "voice-transcription",
        path: "/voice-transcription",
        labelKey: "nav.voiceTranscription",
        icon: <Mic size={18} />,
      },
      {
        key: "appearance",
        path: "/settings/appearance",
        labelKey: "nav.appearance",
        icon: <Paintbrush size={18} />,
      },
      {
        key: "plugins",
        path: "/plugins",
        labelKey: "nav.plugins",
        icon: <Package size={18} />,
      },
      {
        key: "debug",
        path: "/debug",
        labelKey: "nav.debug",
        icon: <Bug size={18} />,
      },
    ],
  },
  {
    key: "about",
    labelKey: null,
    items: [
      {
        key: "about",
        path: "/about",
        labelKey: "nav.about",
        icon: <Info size={18} />,
      },
    ],
  },
];
