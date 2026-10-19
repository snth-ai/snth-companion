// Inline icons: 16 px grid, 1.5 px strokes, drawn for this app.

import type { ReactNode } from "react";

function Svg({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <svg className={`icon${className ? ` ${className}` : ""}`} viewBox="0 0 16 16" aria-hidden="true" focusable="false">
      {children}
    </svg>
  );
}

export const IconStatus = () => (
  <Svg>
    <path d="M8 13.4S2.3 10 2.3 6.1A3.1 3.1 0 0 1 8 4.4a3.1 3.1 0 0 1 5.7 1.7C13.7 10 8 13.4 8 13.4Z" />
  </Svg>
);

export const IconGlobe = () => (
  <Svg>
    <circle cx="8" cy="8" r="6" />
    <path d="M2 8h12M8 2c1.9 1.7 2.8 3.7 2.8 6S9.9 12.3 8 14c-1.9-1.7-2.8-3.7-2.8-6S6.1 3.7 8 2Z" />
  </Svg>
);

export const IconPointer = () => (
  <Svg>
    <path d="M3.5 2.5 12.8 7.2 8.9 8.6 7.3 12.7 3.5 2.5Z" />
    <path d="m9.3 9.4 3.2 3.6" />
  </Svg>
);

export const IconFolder = () => (
  <Svg>
    <path d="M2 4.5A1.5 1.5 0 0 1 3.5 3h2.7l1.5 1.7h4.8A1.5 1.5 0 0 1 14 6.2v5.3a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 2 11.5v-7Z" />
  </Svg>
);

export const IconApps = () => (
  <Svg>
    <rect x="2.5" y="2.5" width="4.5" height="4.5" rx="1.2" />
    <rect x="9" y="2.5" width="4.5" height="4.5" rx="1.2" />
    <rect x="2.5" y="9" width="4.5" height="4.5" rx="1.2" />
    <rect x="9" y="9" width="4.5" height="4.5" rx="1.2" />
  </Svg>
);

export const IconCookie = () => (
  <Svg>
    <path d="M8 2a6 6 0 1 0 6 6 2.6 2.6 0 0 1-3-3 2.6 2.6 0 0 1-3-3Z" />
    <path d="M6 7.3h.01M9.3 10.3h.01M5.8 10.6h.01" strokeWidth="2" />
  </Svg>
);

export const IconList = () => (
  <Svg>
    <path d="M5.5 4h8M5.5 8h8M5.5 12h8" />
    <path d="M2.5 4h.01M2.5 8h.01M2.5 12h.01" strokeWidth="2" />
  </Svg>
);

export const IconSliders = () => (
  <Svg>
    <path d="M2 4.5h5.5M11 4.5h3M2 11.5h3M8.5 11.5H14" />
    <circle cx="9.2" cy="4.5" r="1.7" />
    <circle cx="6.8" cy="11.5" r="1.7" />
  </Svg>
);

export const IconPause = () => (
  <Svg>
    <path d="M5.5 3.5v9M10.5 3.5v9" strokeWidth="2.2" />
  </Svg>
);

export const IconPlay = () => (
  <Svg>
    <path d="M5 3.3v9.4a.5.5 0 0 0 .8.4l7-4.7a.5.5 0 0 0 0-.8l-7-4.7a.5.5 0 0 0-.8.4Z" fill="currentColor" stroke="none" />
  </Svg>
);

export const IconCheck = () => (
  <Svg>
    <path d="m3.5 8.5 3 3 6-7" />
  </Svg>
);

export const IconX = () => (
  <Svg>
    <path d="m4 4 8 8M12 4l-8 8" />
  </Svg>
);

export const IconPlus = () => (
  <Svg>
    <path d="M8 3v10M3 8h10" />
  </Svg>
);

export const IconChevron = () => (
  <Svg className="chev">
    <path d="m6 3.5 4.5 4.5L6 12.5" />
  </Svg>
);

export const IconWarning = () => (
  <Svg>
    <path d="M8 2.3 14.2 13H1.8L8 2.3Z" />
    <path d="M8 6.5v3.2M8 11.3h.01" />
  </Svg>
);

export const IconInfo = () => (
  <Svg>
    <circle cx="8" cy="8" r="6" />
    <path d="M8 7.3v3.7M8 5h.01" />
  </Svg>
);

export const IconLink = () => (
  <Svg>
    <path d="M6.7 9.3a2.8 2.8 0 0 0 4 0l2-2a2.8 2.8 0 0 0-4-4l-.700.7" />
    <path d="M9.3 6.7a2.8 2.8 0 0 0-4 0l-2 2a2.8 2.8 0 0 0 4 4l.700-.700" />
  </Svg>
);

export const IconUpdate = () => (
  <Svg>
    <path d="M8 2.5v7M5 6.8l3 3 3-3" />
    <path d="M2.5 10.5v1.5A1.5 1.5 0 0 0 4 13.5h8a1.5 1.5 0 0 0 1.5-1.5v-1.5" />
  </Svg>
);

/** The command line: a prompt and a cursor. */
export const IconTerminal = () => (
  <Svg>
    <rect x="2" y="3" width="12" height="10" rx="1.5" />
    <path d="m4.8 6.3 1.9 1.7-1.9 1.7M8.4 10h2.8" />
  </Svg>
);

export const IconBlock = () => (
  <Svg>
    <circle cx="8" cy="8" r="6" />
    <path d="m3.8 3.8 8.4 8.4" />
  </Svg>
);
