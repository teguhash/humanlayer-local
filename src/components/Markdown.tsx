import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

/** Renders a Markdown document. Raw HTML in the source is not rendered. */
export function Markdown(props: { text: string }) {
  return (
    <div className="md">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          a: ({ href, children }) => (
            <a href={href} target="_blank" rel="noreferrer">
              {children}
            </a>
          ),
        }}
      >
        {props.text}
      </ReactMarkdown>
    </div>
  );
}
