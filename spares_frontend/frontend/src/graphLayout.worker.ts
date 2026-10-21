// Lays out the graph page's notes, which for thousands of notes takes long enough to freeze the page
import { layOut, type LayoutRequest } from './graphLayout';

self.onmessage = (e: MessageEvent<LayoutRequest>) => {
  self.postMessage(layOut(e.data));
};
