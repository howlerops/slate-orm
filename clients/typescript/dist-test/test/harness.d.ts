import { Client, type Identity } from "../src/index.js";
export declare const CONFIG = "\n[listen]\naddress = \"127.0.0.1:0\"\n\n[auth]\nmode = \"trusted-header\"\n\n[storage]\nbackend = \"memory\"\n\n[[tables]]\nname = \"docs\"\nid = 1\ncolumns = [\n  { name = \"id\",   type = \"u64\" },\n  { name = \"kind\", type = \"str\" },\n  { name = \"size\", type = \"i64\" },\n]\nprimary_key = [\"id\"]\n\n[[security.grants]]\nrole = \"app\"\ntables = [\"docs\"]\nactions = [\"everything\"]\n";
export interface Serving {
    readonly address: string;
    client(identity?: Identity): Client;
    stop(): void;
}
/** Run a daemon on an ephemeral port and wait for it to be listening. */
export declare function start(extra?: string): Promise<Serving>;
