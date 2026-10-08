// Ribbon icons — hand-drawn SVG, flat with subtle gradients: the colourful
// Navicat-style toolbar layer of the design system.

const PLUS_BADGE = (
  <>
    <circle cx="24.5" cy="24.5" r="6" fill="url(#mg-plus-a)" stroke="#ffffff" strokeWidth="1.6" />
    <path
      d="M24.5 21.7v5.6M21.7 24.5h5.6"
      stroke="#ffffff"
      strokeWidth="1.8"
      strokeLinecap="round"
    />
  </>
);

const PLUS_GRADIENT = (
  <linearGradient id="mg-plus-a" x1="0" y1="0" x2="0" y2="1">
    <stop offset="0" stopColor="#7ce38b" />
    <stop offset="1" stopColor="#1f9d4d" />
  </linearGradient>
);

export function IconConnection() {
  return (
    <svg width="30" height="30" viewBox="0 0 32 32" aria-hidden="true">
      <defs>
        <linearGradient id="mg-conn-a" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#7cbcff" />
          <stop offset="1" stopColor="#0b64d0" />
        </linearGradient>
        {PLUS_GRADIENT}
      </defs>
      <path
        d="M5.5 8v12.5c0 2.1 4.3 3.8 9.5 3.8s9.5-1.7 9.5-3.8V8"
        fill="url(#mg-conn-a)"
      />
      <path
        d="M5.5 14.2c0 2.1 4.3 3.8 9.5 3.8s9.5-1.7 9.5-3.8"
        fill="none"
        stroke="#eaf4ff"
        strokeWidth="1.4"
        opacity="0.9"
      />
      <ellipse
        cx="15"
        cy="8"
        rx="9.5"
        ry="3.8"
        fill="#a8d4ff"
        stroke="#0b64d0"
        strokeWidth="1.4"
      />
      {PLUS_BADGE}
    </svg>
  );
}

export function IconNewQuery() {
  return (
    <svg width="30" height="30" viewBox="0 0 32 32" aria-hidden="true">
      <defs>
        {PLUS_GRADIENT}
      </defs>
      <path
        d="M8.5 4h10.2l6.3 6.3V27a1.5 1.5 0 0 1-1.5 1.5H8.5A1.5 1.5 0 0 1 7 27V5.5A1.5 1.5 0 0 1 8.5 4Z"
        fill="#fdfefe"
        stroke="#b9c6d4"
        strokeWidth="1.3"
      />
      <path
        d="M18.7 4v6.3h6.3"
        fill="#dbe7f4"
        stroke="#b9c6d4"
        strokeWidth="1.3"
        strokeLinejoin="round"
      />
      <path
        d="M11.5 14.5h9M11.5 18h9M11.5 21.5h5.5"
        stroke="#5aa9ff"
        strokeWidth="1.7"
        strokeLinecap="round"
      />
      {PLUS_BADGE}
    </svg>
  );
}

export function IconTable() {
  return (
    <svg width="30" height="30" viewBox="0 0 32 32" aria-hidden="true">
      <defs>
        <linearGradient id="mg-tbl-a" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#6fb6ff" />
          <stop offset="1" stopColor="#0b64d0" />
        </linearGradient>
      </defs>
      <rect
        x="5"
        y="6.5"
        width="22"
        height="19"
        rx="2.6"
        fill="url(#mg-tbl-a)"
        stroke="#0b64d0"
        strokeWidth="1.2"
      />
      <path
        d="M5.6 12.3h20.8v-3.2a2 2 0 0 0-2-2H7.6a2 2 0 0 0-2 2z"
        fill="#0b64d0"
        opacity="0.55"
      />
      <path
        d="M5.6 12.6h20.8M5.6 17.4h20.8M5.6 21.9h20.8M12.6 12.6v12.3M19.4 12.6v12.3"
        stroke="#eaf4ff"
        strokeWidth="1.25"
      />
    </svg>
  );
}

export function IconView() {
  return (
    <svg width="30" height="30" viewBox="0 0 32 32" aria-hidden="true">
      <defs>
        <linearGradient id="mg-view-a" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#8ec6ff" />
          <stop offset="1" stopColor="#2f7fd6" />
        </linearGradient>
      </defs>
      <rect
        x="9"
        y="5.5"
        width="17"
        height="12.5"
        rx="2"
        fill="#cfe4fb"
        stroke="#8fb4dc"
        strokeWidth="1.1"
      />
      <path d="M9.6 9.7h15.8M15.8 9.7v8.3M21.4 9.7v8.3" stroke="#8fb4dc" strokeWidth="1" />
      <rect
        x="5.5"
        y="12.5"
        width="18.5"
        height="14"
        rx="2.2"
        fill="url(#mg-view-a)"
        stroke="#1c5fa8"
        strokeWidth="1.2"
      />
      <path
        d="M6 16.9h17.6M6 21.4h17.6M12.6 16.9v9.6M19.2 16.9v9.6"
        stroke="#eaf4ff"
        strokeWidth="1.2"
      />
    </svg>
  );
}

export function IconObjects() {
  return (
    <svg width="30" height="30" viewBox="0 0 32 32" aria-hidden="true">
      <path
        d="M8 4.5h16a1.5 1.5 0 0 1 1.5 1.5v20A1.5 1.5 0 0 1 24 27.5H8A1.5 1.5 0 0 1 6.5 26V6A1.5 1.5 0 0 1 8 4.5Z"
        fill="#fdfefe"
        stroke="#b9c6d4"
        strokeWidth="1.3"
      />
      {[11.5, 16.5, 21.5].map((y) => (
        <g key={y}>
          <circle cx="11.5" cy={y} r="1.5" fill="#2f96ff" />
          <path
            d={`M15 ${y}h7.5`}
            stroke="#5aa9ff"
            strokeWidth="1.6"
            strokeLinecap="round"
          />
        </g>
      ))}
    </svg>
  );
}

export function IconRefresh() {
  return (
    <svg width="30" height="30" viewBox="0 0 32 32" aria-hidden="true">
      <defs>
        <linearGradient id="mg-ref-a" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#7fb8f2" />
          <stop offset="1" stopColor="#3b74b8" />
        </linearGradient>
      </defs>
      <path
        d="M26 16a10 10 0 1 1-2.9-7"
        fill="none"
        stroke="url(#mg-ref-a)"
        strokeWidth="3"
        strokeLinecap="round"
      />
      <path d="M23.8 3.6l2.9 6-6.4 1.2z" fill="#3b74b8" />
    </svg>
  );
}

export function IconSearch() {
  return (
    <svg width="30" height="30" viewBox="0 0 32 32" aria-hidden="true">
      <defs>
        <linearGradient id="mg-sr-a" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#6fb6ff" />
          <stop offset="1" stopColor="#0b64d0" />
        </linearGradient>
      </defs>
      <circle
        cx="14"
        cy="13.5"
        r="7.8"
        fill="rgba(111,182,255,0.22)"
        stroke="url(#mg-sr-a)"
        strokeWidth="2.6"
      />
      <path
        d="M11 10.2a4.6 4.6 0 0 1 3.4-1.6"
        fill="none"
        stroke="#ffffff"
        strokeWidth="1.6"
        strokeLinecap="round"
        opacity="0.9"
      />
      <path
        d="M20 19.6l6.6 6.6"
        stroke="#5f6b78"
        strokeWidth="3.6"
        strokeLinecap="round"
      />
    </svg>
  );
}

export function IconTools() {
  return (
    <svg width="30" height="30" viewBox="0 0 32 32" aria-hidden="true">
      <defs>
        <linearGradient id="mg-tool-a" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#c3ceda" />
          <stop offset="1" stopColor="#66727f" />
        </linearGradient>
      </defs>
      <g transform="rotate(45 16 16)">
        <path
          d="M12.4 12.5h7.2v11.4a3.6 3.6 0 0 1-7.2 0z"
          fill="url(#mg-tool-a)"
          stroke="#4a5763"
          strokeWidth="1.2"
        />
        <path
          d="M12.4 4.4h7.2v8.1h-7.2z"
          fill="url(#mg-tool-a)"
          stroke="#4a5763"
          strokeWidth="1.2"
        />
        <circle cx="16" cy="7" r="2.5" fill="#f2f5f8" stroke="#4a5763" strokeWidth="0.8" />
      </g>
    </svg>
  );
}

export function IconUsers() {
  return (
    <svg width="30" height="30" viewBox="0 0 32 32" aria-hidden="true">
      <defs>
        <linearGradient id="mg-users-a" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#ffcf6b" />
          <stop offset="1" stopColor="#e68200" />
        </linearGradient>
        <linearGradient id="mg-users-b" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#8ec9ff" />
          <stop offset="1" stopColor="#0b64d0" />
        </linearGradient>
      </defs>
      <circle cx="12" cy="11" r="5" fill="url(#mg-users-a)" />
      <path
        d="M3.5 25.5c0-4.4 3.8-7.5 8.5-7.5s8.5 3.1 8.5 7.5v1.5h-17v-1.5z"
        fill="url(#mg-users-a)"
      />
      <circle cx="22.5" cy="13" r="3.6" fill="url(#mg-users-b)" />
      <path
        d="M17.5 25.5v-1c0-3 2.4-5.4 5.5-5.9a8.9 8.9 0 0 1 5.5 3.4v3.5h-11z"
        fill="url(#mg-users-b)"
      />
    </svg>
  );
}

export function IconBackup() {
  return (
    <svg width="30" height="30" viewBox="0 0 32 32" aria-hidden="true">
      <defs>
        <linearGradient id="mg-bak-db" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#7cbcff" />
          <stop offset="1" stopColor="#0b64d0" />
        </linearGradient>
        <linearGradient id="mg-bak-arrow" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#7ce38b" />
          <stop offset="1" stopColor="#1f9d4d" />
        </linearGradient>
      </defs>
      <path
        d="M4.5 8v12.5c0 2 3.4 3.6 7.5 3.6s7.5-1.6 7.5-3.6V8"
        fill="url(#mg-bak-db)"
      />
      <path
        d="M4.5 14.2c0 2 3.4 3.6 7.5 3.6s7.5-1.6 7.5-3.6"
        fill="none"
        stroke="#eaf4ff"
        strokeWidth="1.3"
        opacity="0.9"
      />
      <ellipse
        cx="12"
        cy="8"
        rx="7.5"
        ry="3.2"
        fill="#a8d4ff"
        stroke="#0b64d0"
        strokeWidth="1.3"
      />
      <path
        d="M24 6.5v11.5"
        stroke="url(#mg-bak-arrow)"
        strokeWidth="3.2"
        strokeLinecap="round"
      />
      <path
        d="M20.4 15.4h7.2L24 21z"
        fill="#1f9d4d"
        stroke="#1f9d4d"
        strokeWidth="1.2"
        strokeLinejoin="round"
      />
      <path
        d="M19.5 25.5h9"
        stroke="#8fb4dc"
        strokeWidth="1.6"
        strokeLinecap="round"
        opacity="0.9"
      />
    </svg>
  );
}
