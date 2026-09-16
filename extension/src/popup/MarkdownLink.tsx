import type { AnchorHTMLAttributes } from "react";

// Links must open in a real browser tab, not navigate the popup itself away (which would close it).
// Same role as MarkdownLink in desktop/src/components/MessageBubble.tsx, swapping Tauri's openUrl
// for chrome.tabs.create.
export function PopupMarkdownLink(props: AnchorHTMLAttributes<HTMLAnchorElement>) {
  const { href, children, ...rest } = props;
  return (
    <a
      {...rest}
      href={href}
      onClick={(event) => {
        event.preventDefault();
        if (href) chrome.tabs.create({ url: href });
      }}
    >
      {children}
    </a>
  );
}
