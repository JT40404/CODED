/**
 * Metadata upload. The browser posts to your own endpoint so storage keys
 * never reach the client; the endpoint pins the image and the JSON to IPFS.
 */
import type { UploadMetadata } from "./launch";

/** Browser side: POST multipart to `endpoint`, expect `{ uri }` back. */
export function uploadViaEndpoint(endpoint: string): UploadMetadata {
  return async (m) => {
    const form = new FormData();
    form.append("file", m.image);
    form.append("name", m.name);
    form.append("symbol", m.symbol);
    form.append("description", m.description);
    if (m.website) form.append("website", m.website);
    if (m.twitter) form.append("twitter", m.twitter);
    if (m.telegram) form.append("telegram", m.telegram);
    const res = await fetch(endpoint, { method: "POST", body: form });
    if (!res.ok) throw new Error(`Metadata upload failed (${res.status}). Check the image is under 4 MB.`);
    const { uri } = (await res.json()) as { uri: string };
    return uri;
  };
}

/**
 * Server side (e.g. a Vercel/Cloudflare function at /api/metadata).
 * Pins with Pinata; set PINATA_JWT in the environment.
 */
export async function handleMetadataUpload(req: Request, pinataJwt: string): Promise<Response> {
  const form = await req.formData();
  const file = form.get("file");
  if (!(file instanceof Blob) || file.size > 4 * 1024 * 1024) {
    return Response.json({ error: "Image must be a file under 4 MB." }, { status: 400 });
  }
  const pin = async (body: FormData) => {
    const r = await fetch("https://api.pinata.cloud/pinning/pinFileToIPFS", {
      method: "POST",
      headers: { Authorization: `Bearer ${pinataJwt}` },
      body,
    });
    if (!r.ok) throw new Error(`Pinata ${r.status}`);
    return ((await r.json()) as { IpfsHash: string }).IpfsHash;
  };
  const imgForm = new FormData();
  imgForm.append("file", file);
  const imageCid = await pin(imgForm);

  const str = (k: string) => (typeof form.get(k) === "string" ? (form.get(k) as string) : undefined);
  const json = {
    name: str("name"),
    symbol: str("symbol"),
    description: str("description") ?? "",
    image: `https://ipfs.io/ipfs/${imageCid}`,
    showName: true,
    website: str("website"),
    twitter: str("twitter"),
    telegram: str("telegram"),
  };
  const jsonForm = new FormData();
  jsonForm.append("file", new Blob([JSON.stringify(json)], { type: "application/json" }), "metadata.json");
  const jsonCid = await pin(jsonForm);
  return Response.json({ uri: `https://ipfs.io/ipfs/${jsonCid}` });
}
