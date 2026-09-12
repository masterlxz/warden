/** Small inline SVG icon set — kept hand-rolled rather than pulling in an icon library, in the
 * same minimal-dependency spirit as the rest of the app. Outline icons share one stroke style
 * (24x24, round caps/joins) so they read as one family; `LogoMark` is the one filled exception —
 * it's the app's brand mark, not a UI action icon. */

function outlineProps() {
  return {
    viewBox: "0 0 24 24",
    fill: "none" as const,
    stroke: "currentColor",
    strokeWidth: 1.8,
    strokeLinecap: "round" as const,
    strokeLinejoin: "round" as const,
  };
}

/** Warden's brand mark — a shield, filled with the accent gradient. Used at the top of the
 * sidebar, on the assistant's avatar, and in the empty-state hero. `size` controls both
 * dimensions; the gradient id is namespaced per instance so multiple copies on one page don't
 * collide. */
export function LogoMark({ size = 22, className }: { size?: number; className?: string }) {
  const gradientId = `warden-logo-gradient-${size}`;
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" className={className} aria-hidden="true">
      <defs>
        <linearGradient id={gradientId} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0%" stopColor="var(--color-accent)" />
          <stop offset="100%" stopColor="var(--color-accent-dark)" />
        </linearGradient>
      </defs>
      <path
        d="M12 2.5l7.5 3v5.2c0 4.9-3.2 9.2-7.5 10.8-4.3-1.6-7.5-5.9-7.5-10.8V5.5l7.5-3z"
        fill={`url(#${gradientId})`}
      />
      <path d="M8.7 12.1l2.2 2.2 4.4-4.6" stroke="white" strokeWidth={1.6} strokeLinecap="round" strokeLinejoin="round" fill="none" />
    </svg>
  );
}

export function PlusIcon({ size = 18 }: { size?: number }) {
  return (
    <svg width={size} height={size} {...outlineProps()} aria-hidden="true">
      <path d="M12 5v14M5 12h14" />
    </svg>
  );
}

export function ChartIcon({ size = 18 }: { size?: number }) {
  return (
    <svg width={size} height={size} {...outlineProps()} aria-hidden="true">
      <path d="M4 20V10M11 20V4M18 20v-7" />
      <path d="M3 20h18" />
    </svg>
  );
}

export function SettingsIcon({ size = 18 }: { size?: number }) {
  return (
    <svg width={size} height={size} {...outlineProps()} aria-hidden="true">
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 13a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V19a2 2 0 0 1-4 0v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H4a2 2 0 0 1 0-4h.09a1.65 1.65 0 0 0 1.51-1 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H10a1.65 1.65 0 0 0 1-1.51V4a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V10a1.65 1.65 0 0 0 1.51 1H20a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </svg>
  );
}

export function SyncIcon({ size = 18 }: { size?: number }) {
  return (
    <svg width={size} height={size} {...outlineProps()} aria-hidden="true">
      <path d="M4 12a8 8 0 0 1 14.5-4.6M20 12a8 8 0 0 1-14.5 4.6" />
      <path d="M18.5 3v4.4h-4.4M5.5 21v-4.4h4.4" />
    </svg>
  );
}

export function VaultIcon({ size = 18 }: { size?: number }) {
  return (
    <svg width={size} height={size} {...outlineProps()} aria-hidden="true">
      <path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20" />
      <path d="M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2Z" />
    </svg>
  );
}

export function DevicesIcon({ size = 18 }: { size?: number }) {
  return (
    <svg width={size} height={size} {...outlineProps()} aria-hidden="true">
      <rect x="2" y="3" width="20" height="7" rx="1.8" />
      <rect x="2" y="14" width="20" height="7" rx="1.8" />
      <path d="M6 6.5h.01M6 17.5h.01" />
    </svg>
  );
}

export function SendIcon({ size = 18 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <path d="M12 19V5M12 5l-6 6M12 5l6 6" stroke="currentColor" strokeWidth={2.2} strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

export function AttachIcon({ size = 18 }: { size?: number }) {
  return (
    <svg width={size} height={size} {...outlineProps()} aria-hidden="true">
      <path d="M18.5 6.5v9a4.5 4.5 0 0 1-9 0v-10a3 3 0 0 1 6 0v9a1.5 1.5 0 0 1-3 0v-8.5" />
    </svg>
  );
}

export function CloseIcon({ size = 14 }: { size?: number }) {
  return (
    <svg width={size} height={size} {...outlineProps()} aria-hidden="true">
      <path d="M18 6 6 18M6 6l12 12" />
    </svg>
  );
}

export function MicIcon({ size = 18 }: { size?: number }) {
  return (
    <svg width={size} height={size} {...outlineProps()} aria-hidden="true">
      <rect x="9" y="2" width="6" height="12" rx="3" />
      <path d="M5 10.5a7 7 0 0 0 14 0M12 17.5V22M8.5 22h7" />
    </svg>
  );
}

export function SpeakerIcon({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} {...outlineProps()} aria-hidden="true">
      <path d="M4 9.5h3.5L12 5.5v13l-4.5-4H4z" strokeLinejoin="round" />
      <path d="M16 8.5a5 5 0 0 1 0 7M18.5 6a8.5 8.5 0 0 1 0 12" />
    </svg>
  );
}

export function StopIcon({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} {...outlineProps()} aria-hidden="true">
      <rect x="6" y="6" width="12" height="12" rx="2" />
    </svg>
  );
}

/** Points left by default (the sidebar's collapse toggle rotates it 180° when collapsed, so it
 * points right — "expand" — instead). */
export function ChevronIcon({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} {...outlineProps()} aria-hidden="true">
      <path d="M15 6l-6 6 6 6" />
    </svg>
  );
}
