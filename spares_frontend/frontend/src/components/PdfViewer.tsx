interface Props { url: string }

export default function PdfViewer({ url }: Props) {
  return (
    <div>
      <embed src={url} type="application/pdf" width="100%" height="600px" />
      <p style={{ marginTop: 8, fontSize: 12, color: '#666' }}>
        <a href={url} target="_blank" rel="noreferrer">Open PDF in new tab</a>
      </p>
    </div>
  );
}
