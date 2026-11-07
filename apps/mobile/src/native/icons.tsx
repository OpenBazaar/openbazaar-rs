import { SvgXml } from "react-native-svg";

// The same stroke icons the shop draws in Marketplace.tsx.
type IconName = "settings" | "wallet" | "compass" | "heart" | "bag" | "book" | "globe";

const PATHS: Record<IconName, string> = {
  settings: `<path d="m9 3-1 3-3 1-2 5 2 5 3 1 1 3h6l1-3 3-1 2-5-2-5-3-1-1-3Z"/><circle cx="12" cy="12" r="3"/>`,
  wallet: `<rect x="3" y="6" width="18" height="14" rx="2"/><path d="M3 8V5a2 2 0 0 1 2-2h12v3M21 11h-6v5h6"/>`,
  compass: `<circle cx="12" cy="12" r="9"/><path d="m16 8-2.5 5.5L8 16l2.5-5.5Z"/>`,
  heart: `<path d="M20.8 4.6a5.5 5.5 0 0 0-7.8 0L12 5.7l-1.1-1.1a5.5 5.5 0 0 0-7.8 7.8L12 21l8.8-8.6a5.5 5.5 0 0 0 0-7.8Z"/>`,
  bag: `<path d="M5 7h14l1 14H4L5 7Z"/><path d="M9 8V6a3 3 0 0 1 6 0v2"/>`,
  book: `<path d="M4 4h6l2 2 2-2h6v15h-6l-2 2-2-2H4Z"/><path d="M12 6v15"/>`,
  globe: `<circle cx="12" cy="12" r="9"/><ellipse cx="12" cy="12" rx="4" ry="9"/><path d="M3 12h18"/>`,
};

export function NativeIcon({ name, size = 22, color, filled = false }: { name: IconName; size?: number; color: string; filled?: boolean }) {
  const fill = filled ? color : "none";
  const xml = `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" viewBox="0 0 24 24" fill="${fill}" stroke="${color}" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">${PATHS[name]}</svg>`;
  return <SvgXml xml={xml} width={size} height={size} />;
}

export type { IconName };
