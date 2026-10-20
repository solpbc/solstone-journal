import { Message, Term } from "./ast.js";
/**
 * Fluent Resource is a structure storing parsed localization entries.
 */
export declare class FluentResource {
    /** @ignore */
    body: Array<Message | Term>;
    constructor(source: string);
}
