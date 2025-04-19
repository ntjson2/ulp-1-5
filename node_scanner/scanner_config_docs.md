# Documentation for scanner_config.json

This file configures the `scan_pairs.js` script to fetch liquidity pool data from various DEX protocols across different blockchain networks using The Graph protocol.

## Root Level

-   **`global`** (`object`): Defines settings applicable across all chains and protocols unless overridden locally.
    -   **`pageSize`** (`number`): The default number of items (pools) to request in each GraphQL query page. Subgraphs often have a maximum limit (typically 1000).

-   **`chains`** (`object`): A container where each key represents a blockchain network (e.g., "optimism").

## Chain Level (`chains.<chainName>`)

Example Key: `"optimism"` (`object`)

-   Configuration specific to a blockchain network (e.g., Optimism).
    -   **`chainId`** (`number`, Optional): The numerical Chain ID for the network (e.g., 10 for Optimism). Useful for reference or if your bot needs it later.
    -   **`protocols`** (`object`): A container where each key represents a DEX protocol on this specific chain (e.g., "uniswap_v3").

## Protocol Level (`chains.<chainName>.protocols.<protocolName>`)

Example Key: `"uniswap_v3"` (`object`)

-   Configuration for a specific DEX protocol on a specific chain.
    -   **`displayName`** (`string`): A user-friendly name for logging and output files (e.g., "Uniswap V3").
    -   **`subgraphId`** (`string`): The unique identifier (Qm... hash or Studio ID like `Cghf4...` or `A4Y1...`) of the specific subgraph deployment to query. This should typically be the ID for the Graph Gateway / Subgraph Studio version, found on The Graph Explorer for the desired network.
        -   *Example Uniswap V3 (Optimism)*: `Cghf4LfVqPiFw6fp6Y5X5Ubc8UpmUhSfJL82zwiBFLaj` (as provided by user).
        -   *Example Velodrome V2 (Optimism)*: `A4Y1A82YhSLTn998BVVELC8eWzhi992k4ZitByvssxqA` (previously identified V2 subgraph - recommended to verify).
    -   **`useApiKey`** (`boolean`):
        -   `true`: Use The Graph Gateway endpoint (e.g., `https://gateway.thegraph.com/api/[api-key]/subgraphs/id/...`). This requires a valid `GRAPH_API_KEY` defined in the `.env` file. This is the recommended and more reliable method.
        -   `false`: Attempt to use a public hosted service endpoint (e.g., `https://api.thegraph.com/subgraphs/id/...`). These endpoints may be less reliable or deprecated.
    -   **`poolFieldName`** (`string`): **Crucial.** The exact name of the field within the GraphQL query's response data that contains the array of pools. This **must match the subgraph's schema**.
        -   *Example Uniswap V3*: `"pools"`
        -   *Example Velodrome V2*: Often `"liquidityPools"` (verify schema).
    -   **`query`** (`string`): The complete GraphQL query string used to fetch pools from this specific subgraph.
        -   It **must** accept `$pageSize: Int!` and `$lastId: ID!` as GraphQL variables for pagination.
        -   It **should** use `orderBy: id, orderDirection: asc` and `where: { id_gt: $lastId }` for reliable pagination based on the pool's address/ID.
        -   It **must** request the pool `id` (which is typically the pool's contract address).
        -   It **must** request the identifiers for the two tokens in the pool. The structure depends on the subgraph schema:
            -   Common pattern: `token0 { id }`, `token1 { id }`
            -   Alternative (e.g., some Velodrome V2 forks): `inputTokens { id }` (verify the exact field names and structure).
        -   It can optionally request other useful static pool data available directly on the pool entity (e.g., `feeTier` for Uniswap V3, potentially `stableFee`, `volatileFee` for Velodrome V2 - verify availability).
        -   Newlines within the query string must be represented as `\n` in the JSON value.
        -   **IMPORTANT:** You MUST verify the query structure (especially token and fee fields) against the actual schema definition for the specified `subgraphId` on The Graph Explorer.

## Example Protocol Queries (Illustrative - Verify Schemas)

### Uniswap V3 Style Query

```graphql
query GetPools($pageSize: Int!, $lastId: ID!) {
  pools(first: $pageSize, orderBy: id, orderDirection: asc, where: { id_gt: $lastId }) {
    id          # Pool address
    token0 { id } # Address of token 0
    token1 { id } # Address of token 1
    feeTier     # e.g., 100, 500, 3000, 10000
  }
}