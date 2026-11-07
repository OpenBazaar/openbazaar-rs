import { useEffect, useState } from "react";
import { Image, View, type ImageStyle, type StyleProp } from "react-native";

import { bundledPhoto } from "./bundledGoods.ts";
import { pairedImageUri } from "./media.ts";
import { SOFT } from "./ui.tsx";

function usePairedImage(hash: string | undefined): string | null {
  const [uri, setUri] = useState<string | null>(null);
  useEffect(() => {
    if (!hash) return;
    let active = true;
    setUri(null);
    pairedImageUri(hash).then(
      (next) => active && setUri(next),
      () => active && setUri(null),
    );
    return () => {
      active = false;
    };
  }, [hash]);
  return uri;
}

/// A listing photo. Signed images load through the paired node. Demo goods
/// with no hash use the picture shipped in the app.
export function ListingPhoto({ hash, title, index = 0, style }: { hash?: string; title: string; index?: number; style: StyleProp<ImageStyle> }) {
  const uri = usePairedImage(hash);
  if (hash) {
    if (!uri) return <View style={[style, { backgroundColor: SOFT }]} accessibilityLabel={title} />;
    return <Image source={{ uri }} style={style} accessibilityLabel={title} />;
  }
  const bundled = bundledPhoto(title, index);
  if (bundled) return <Image source={bundled} style={style} accessibilityLabel={title} />;
  return <View style={[style, { backgroundColor: SOFT }]} accessibilityLabel={title} />;
}
