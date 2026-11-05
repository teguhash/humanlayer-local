import { useEffect, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { openUrl } from "@tauri-apps/plugin-opener";

/** Renders a Markdown document. Raw HTML in the source is not rendered.
 * Links open in the browser. Relative image paths are loaded through
 * `resolveImage` (the docs folder); remote images are not loaded. */
export function Markdown(props: { text: string; resolveImage?: (src: string) => Promise<string> }) {
  return (
    <div className="md">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          a: ({ href, children }) => (
            <a
              href={href}
              onClick={(e) => {
                e.preventDefault();
                if (href && /^https?:\/\//.test(href)) openUrl(href).catch(() => {});
              }}
            >
              {children}
            </a>
          ),
          img: ({ src, alt }) => <LocalImage src={typeof src === "string" ? src : ""} alt={alt ?? ""} resolve={props.resolveImage} />,
        }}
      >
        {props.text}
      </ReactMarkdown>
    </div>
  );
}

function LocalImage(props: { src: string; alt: string; resolve?: (src: string) => Promise<string> }) {
  const [url, setUrl] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const remote = /^[a-z]+:/i.test(props.src);
  useEffect(() => {
    setUrl(null);
    setFailed(false);
    if (remote || !props.resolve) return;
    props
      .resolve(props.src)
      .then(setUrl)
      .catch(() => setFailed(true));
  }, [props.src, props.resolve, remote]);
  if (remote || failed || !props.resolve)
    return <span className="text-muted-foreground">[image: {props.alt || props.src}]</span>;
  return url ? <img src={url} alt={props.alt} className="max-w-full" /> : null;
}
