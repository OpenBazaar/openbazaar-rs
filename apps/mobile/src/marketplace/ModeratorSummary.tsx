import { useEffect } from "react";
import { shortKey, trackRecordText, type KnownModerator } from "../moderators";

/** Styles for the moderator profile, the store's moderator list, and the checkout picker. */
export function useModeratorStyles() {
  useEffect(() => {
    if (document.querySelector("link[data-ob-moderators]")) return;
    const link = document.createElement("link");
    link.rel = "stylesheet";
    link.href = "moderators.css";
    link.dataset.obModerators = "";
    document.head.appendChild(link);
  }, []);
}

/** What a seller or buyer compares moderators on: the key, the fee, and the verified track record. */
export function ModeratorSummary({ moderator }: { moderator: KnownModerator }) {
  return <div className="ob-moderator-summary">
    <strong>{moderator.name || "Unnamed moderator"}{moderator.builtIn && <span className="ob-moderator-badge">Built-in test moderator</span>}</strong>
    <span className="ob-moderator-facts"><span>Fee {moderator.feeSats.toLocaleString()} sats if a dispute is ruled</span><span>Key <code title={moderator.id}>{shortKey(moderator.id)}</code></span>{moderator.languages.length > 0 && <span>{moderator.languages.join(", ")}</span>}</span>
    <span className="ob-moderator-track">{trackRecordText(moderator)}</span>
    {moderator.description && <p>{moderator.description}</p>}
    {moderator.terms && <details><summary>Terms and policy</summary><p>{moderator.terms}</p></details>}
  </div>;
}
