/** Tauri event names mirrored by the events module in src-tauri/src/bridge.rs. */
export const EVENTS = {
  /** Peer came online; payload is PeerDto. */
  PEER_UP: "peer-up",
  /** Peer went offline; payload is its fingerprint. */
  PEER_DOWN: "peer-down",
  /** Incoming transfer awaiting a user decision; payload is OfferDto. */
  TRANSFER_OFFER: "transfer-offer",
  /** Transfer lifecycle event; payload is TransferEventDto. */
  TRANSFER_EVENT: "transfer-event",
  /** Trusted-device automatic receive started. */
  TRANSFER_AUTOSTART: "transfer-autostart",
  /** Peer avatar cache is ready and should be reloaded. */
  AVATAR_READY: "avatar-ready",
  /** Global hotkey requesting a clipboard send to the selected peer.
   * Shared by send-clipboard and copy-and-send after backend confirmation. */
  HOTKEY_SEND_CLIPBOARD: "hotkey-send-clipboard",
  /** Tray settings action requesting the settings dialog. */
  OPEN_SETTINGS: "open-settings",
} as const;
