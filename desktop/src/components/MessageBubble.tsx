import type { AnchorHTMLAttributes } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { ChatMessage } from "../types";

interface MessageBubbleProps {
  message: ChatMessage;
}

// Links must open in the user's default browser, not navigate the app's own webview away.
function MarkdownLink(props: AnchorHTMLAttributes<HTMLAnchorElement>) {
  const { href, children, ...rest } = props;
  return (
    <a
      {...rest}
      href={href}
      onClick={(event) => {
        event.preventDefault();
        if (href) void openUrl(href);
      }}
    >
      {children}
    </a>
  );
}

function MessageBubble({ message }: MessageBubbleProps) {
  return (
    <div className={`message-bubble message-bubble--${message.role}`}>
      <div className="message-bubble-content">
        <ReactMarkdown remarkPlugins={[remarkGfm]} components={{ a: MarkdownLink }}>
          {message.content}
        </ReactMarkdown>
      </div>
    </div>
  );
}

export default MessageBubble;
