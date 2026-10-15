import { EMPTY_SIGNATURE_LIBRARY, signatureDefaultsKey, validateSignature, type Signature, type SignatureDefaults, type SignatureLibrary } from "../signatures";
const key = "threestrands.demoSignatures";
export function readDemoSignatures(): SignatureLibrary {
  const stored = localStorage.getItem(key);
  return stored ? JSON.parse(stored) : structuredClone(EMPTY_SIGNATURE_LIBRARY);
}
function write(library: SignatureLibrary) { localStorage.setItem(key, JSON.stringify(library)); }
export const demoSignatureClient = {
  async signatureLibrary() { return readDemoSignatures(); },
  async saveSignature(signature: Signature) {
    validateSignature(signature);
    const library = readDemoSignatures();
    library.signatures = [...library.signatures.filter((s) => s.id !== signature.id), signature];
    write(library);
  },
  async deleteSignature(id: string) {
    const library = readDemoSignatures();
    library.signatures = library.signatures.filter((s) => s.id !== id);
    write(library);
  },
  async saveSignatureDefaults(defaults: SignatureDefaults) {
    const library = readDemoSignatures();
    library.defaults = [...library.defaults.filter((d) => d.id !== defaults.id), defaults];
    write(library);
  },
  async deleteSignatureDefaults(account: string, address: string) {
    const library = readDemoSignatures();
    library.defaults = library.defaults.filter((d) => signatureDefaultsKey(d.account, d.address) !== signatureDefaultsKey(account, address));
    write(library);
  },
};
