import type { AnchorHTMLAttributes } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { ChatMessage } from "../types";
import { LogoMark } from "./Icons";

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
  if (message.role === "assistant") {
    return (
      <div className="message-row message-row--assistant">
        <div className="message-avatar">
          <LogoMark size={18} />
        </div>
        <div className="message-assistant-body">
          <div className="message-bubble-content">
            <ReactMarkdown remarkPlugins={[remarkGfm]} components={{ a: MarkdownLink }}>
              {message.content}
            </ReactMarkdown>
          </div>
          {message.usage && <div className="message-bubble-usage">{message.usage.totalTokens} tokens</div>}
        </div>
      </div>
    );
  }

  return (
    <div className="message-row message-row--user">
      <div className="message-bubble message-bubble--user">
        {message.attachments && message.attachments.length > 0 && (
          <div className="message-bubble-attachments">
            {message.attachments.map((attachment, index) => (
              <img key={index} src={`data:${attachment.mimeType};base64,${attachment.data}`} alt="" />
            ))}
          </div>
        )}
        {message.content && (
          <div className="message-bubble-content">
            <ReactMarkdown remarkPlugins={[remarkGfm]} components={{ a: MarkdownLink }}>
              {message.content}
            </ReactMarkdown>
          </div>
        )}
      </div>
    </div>
  );
}

export default MessageBubble;
