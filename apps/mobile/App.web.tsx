// Browser and desktop use the responsive marketplace; native screens remain in App.tsx.
import Marketplace from "./src/marketplace/Marketplace";
import { withHomeNode } from "./src/marketplace/HomeNodeGate";

export default withHomeNode(Marketplace);
