import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

const REMARK_PLUGINS = [remarkGfm];

const BASE_CLASSES =
  "prose prose-invert prose-sm max-w-none " +
  "prose-headings:text-gray-100 prose-headings:font-semibold " +
  "prose-p:text-gray-300 " +
  "prose-strong:text-gray-100 " +
  "prose-code:text-blue-300 prose-code:bg-gray-800 prose-code:px-1 prose-code:rounded prose-code:text-xs " +
  "prose-pre:bg-gray-900 prose-pre:border prose-pre:border-gray-700 " +
  "prose-pre:code:bg-transparent prose-pre:code:px-0 " +
  "prose-a:text-blue-400 hover:prose-a:text-blue-300 " +
  "prose-blockquote:border-gray-700 prose-blockquote:text-gray-400 " +
  "prose-hr:border-gray-700 " +
  "prose-th:text-gray-200 prose-td:text-gray-300 " +
  "prose-li:text-gray-300";

interface MarkdownViewerProps {
  content: string;
  className?: string;
}

export function MarkdownViewer({ content, className }: MarkdownViewerProps) {
  return (
    <div className={className ? `${BASE_CLASSES} ${className}` : BASE_CLASSES}>
      <ReactMarkdown remarkPlugins={REMARK_PLUGINS}>{content}</ReactMarkdown>
    </div>
  );
}
