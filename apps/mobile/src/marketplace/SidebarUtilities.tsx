import type { RefObject } from "react";
import { hasNodeHost, type CurrencyCode } from "../catalog";
import { servedByHomeNode } from "../homeNode/transport";
import { BuildLine, type BuildInfoState } from "./BuildInfo";
import { Icon, NavItem, type Section } from "./Marketplace";
import { TorIndicator } from "./TorStatus";

type Props = {
  guest: boolean;
  nodeAvailable: boolean;
  publicMode: boolean;
  direct: string | null;
  loading: boolean;
  loadError: string;
  section: Section;
  navSection: Section;
  inSettings: boolean;
  settingsOpen: boolean;
  settingsToggleRef: RefObject<HTMLButtonElement | null>;
  settingsMenuRef: RefObject<HTMLElement | null>;
  onToggleSettings: () => void;
  onCloseSettings: () => void;
  onNavigate: (next: Section) => void;
  testnet4: boolean;
  hasProfile: boolean;
  seller: boolean;
  onStartSelling: () => void;
  currency: CurrencyCode;
  onCurrency: (next: CurrencyCode) => void;
  build: BuildInfoState;
  onCopied: (message: string) => void;
  unreadOrders?: number;
};

/** Sidebar footer: orders/wallet, Settings menu, and a compact connection line. */
export function SidebarUtilities({
  guest,
  nodeAvailable,
  publicMode,
  direct,
  loading,
  loadError,
  section,
  navSection,
  inSettings,
  settingsOpen,
  settingsToggleRef,
  settingsMenuRef,
  onToggleSettings,
  onCloseSettings,
  onNavigate,
  testnet4,
  hasProfile,
  seller,
  onStartSelling,
  currency,
  onCurrency,
  build,
  onCopied,
  unreadOrders = 0,
}: Props) {
  const connected = !guest && !loading && !loadError && hasNodeHost();
  const statusLabel = publicMode
    ? "Browsing"
    : direct
      ? "Store browse"
      : !hasNodeHost()
        ? "No node"
        : loading
          ? "Connecting…"
          : loadError
            ? "Offline"
            : "Connected";
  const networkLabel = testnet4 ? "Testnet4" : connected ? "Mainnet" : null;
  const compact = networkLabel ? `${statusLabel} · ${networkLabel}` : statusLabel;
  const tooltip = [compact, build.line].filter(Boolean).join(" · ");

  return <div className="ob-sidebar-utilities">
    {!guest && <nav className="ob-utility-nav ob-main-nav" aria-label="Orders and wallet">
      <NavItem
        icon="paper"
        active={navSection === "orders"}
        onClick={() => onNavigate("orders")}
        count={unreadOrders || undefined}
        countNew={unreadOrders > 0}
        countLabel={unreadOrders ? `${unreadOrders} unread ${unreadOrders === 1 ? "message" : "messages"}` : undefined}
      >Your orders</NavItem>
      <NavItem icon="wallet" active={navSection === "wallet"} onClick={() => onNavigate("wallet")}>Wallet</NavItem>
    </nav>}
    <button ref={settingsToggleRef} className={`ob-nav-item ob-settings-toggle ${inSettings ? "has-active-child" : ""}`} aria-label="Settings" title="Settings" aria-expanded={settingsOpen} aria-controls="ob-settings-nav" onClick={onToggleSettings}><Icon name="settings" size={19}/><span>Settings</span><Icon name="chevron" size={14}/></button>
    <nav ref={settingsMenuRef} id="ob-settings-nav" className="ob-settings-nav" aria-label="Settings" hidden={!settingsOpen} onBlur={e => { if (!e.currentTarget.contains(e.relatedTarget) && e.relatedTarget !== settingsToggleRef.current) onCloseSettings(); }}>
      <div className="ob-settings-menu-title">Settings</div>
      <NavItem icon="shield" active={section === "safety"} onClick={() => onNavigate("safety")}>Content filters</NavItem>
      {!guest && <>
        <NavItem icon="pin" active={section === "addresses"} onClick={() => onNavigate("addresses")}>Addresses</NavItem>
        <NavItem icon="grid" active={section === "store-labels"} onClick={() => onNavigate("store-labels")}>Store labels</NavItem>
        <NavItem icon="globe" active={section === "home-node"} onClick={() => onNavigate("home-node")}>Home node</NavItem>
        {!servedByHomeNode() && <NavItem icon="globe" active={section === "browsers"} onClick={() => onNavigate("browsers")}>Browsers</NavItem>}
        <NavItem icon="bolt" active={section === "lightning"} onClick={() => onNavigate("lightning")}>Lightning</NavItem>
        {testnet4 && <NavItem icon="wallet" active={section === "zcash"} onClick={() => onNavigate("zcash")}>Zcash</NavItem>}
        {testnet4 && <NavItem icon="wallet" active={section === "monero"} onClick={() => onNavigate("monero")}>Monero</NavItem>}
        <NavItem icon="shield" active={section === "tor"} onClick={() => onNavigate("tor")}>Tor</NavItem>
        <NavItem icon="globe" active={section === "follows"} onClick={() => onNavigate("follows")}>Followed stores</NavItem>
      </>}
      {guest
        ? <NavItem icon="home" active={section === "store"} onClick={() => onNavigate("store")}>Start selling</NavItem>
        : hasProfile && (seller
          ? <NavItem icon="home" active={section === "store"} onClick={() => onNavigate("store")}>Store settings</NavItem>
          : <NavItem icon="home" active={false} onClick={onStartSelling}>Start selling</NavItem>)}
      {!guest && <div className="ob-settings-currency"><label className="ob-field">Display currency<select aria-label="Display currency" value={currency} onChange={e => onCurrency(e.target.value as CurrencyCode)}><option value="USD">USD · US dollar</option><option value="EUR">EUR · Euro</option><option value="GBP">GBP · British pound</option><option value="JPY">JPY · Japanese yen</option></select></label><p>Used for price estimates.</p></div>}
      {build.line ? <BuildLine build={build} onCopied={onCopied}/> : null}
    </nav>
    <div className="ob-node-status" title={tooltip}>
      <span className={`ob-status-dot ${guest ? "" : loadError ? "is-offline" : loading ? "is-loading" : ""}`} aria-hidden="true"/>
      <strong>{compact}</strong>
    </div>
    {!loadError && nodeAvailable && <TorIndicator onOpen={() => onNavigate("tor")}/>}
  </div>;
}
