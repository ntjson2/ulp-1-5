// File: scanner_scripts/scan_pairs.js

const { request, gql } = require('graphql-request');
const fs = require('fs');
const path = require('path');
require('dotenv').config({ path: path.resolve(__dirname, '../.env') }); // Load .env from parent dir

// --- Configuration Loading ---
const CONFIG_FILE = path.join(__dirname, 'scanner_config.json');
let CONFIG;
try {
    CONFIG = JSON.parse(fs.readFileSync(CONFIG_FILE, 'utf-8'));
    console.log("✅ Configuration loaded successfully.");
} catch (error) {
    console.error(`❌ Failed to load or parse configuration file ${CONFIG_FILE}: ${error.message}`);
    process.exit(1); // Exit if config is broken
}

const GRAPH_GATEWAY_BASE = 'https://gateway.thegraph.com/api/';
const API_KEY = process.env.GRAPH_API_KEY;
const GLOBAL_PAGE_SIZE = CONFIG.global?.pageSize || 1000; // Use default from config or 1000
const GLOBAL_FETCH_LIMIT = CONFIG.global?.fetchLimit || Infinity;


// --- Helper Functions ---

/**
 * Fetches pools for a specific protocol, respects limits, handles token structures, and extracts symbols.
 * @returns {Promise<{poolsData: object, pairSymbolMap: object}>} - Object containing pool addresses map and pair symbol map.
 */
async function getAllPools(chainName, protocolName) {
    const protocolConfig = CONFIG.chains?.[chainName]?.protocols?.[protocolName];
    const displayName = protocolConfig?.displayName || protocolName;
    const fetchLimit = protocolConfig?.fetchLimit || GLOBAL_FETCH_LIMIT;

    console.log(`\n🌀 Fetching pools for ${displayName} on ${chainName}... (Limit: ${fetchLimit === Infinity ? 'None' : fetchLimit})`);

    if (!protocolConfig || !protocolConfig.subgraphId || !protocolConfig.poolFieldName || !protocolConfig.query) {
        console.error(`❗️ Invalid or incomplete configuration for ${displayName} on ${chainName}. Skipping.`);
        return { poolsData: {}, pairSymbolMap: {} }; // Return empty objects
    }

    const endpoint = protocolConfig.useApiKey
        ? `${GRAPH_GATEWAY_BASE}${API_KEY}/subgraphs/id/${protocolConfig.subgraphId}`
        : `https://api.thegraph.com/subgraphs/id/${protocolConfig.subgraphId}`;

    const headers = {};
    if (protocolConfig.useApiKey) {
        if (!API_KEY) {
            console.warn(`⚠️ API key configured for ${displayName} but not found in .env. Trying unauthenticated.`);
        } else {
            headers['Authorization'] = `Bearer ${API_KEY}`;
        }
    }

    let lastId = '';
    let poolsData = {}; // Map: pairKey -> [poolAddr1, poolAddr2,...]
    let pairSymbolMap = {}; // Map: pairKey -> "SYM0/SYM1" (ordered by sorted address)
    let fetchedCount = 0;
    let page = 1;
    const pageSize = protocolConfig.pageSize || GLOBAL_PAGE_SIZE;
    let limitReached = false;

    while (true) {
        if (fetchedCount >= fetchLimit) {
            console.log(`   ⚠️ Fetch limit (${fetchLimit}) reached or exceeded. Stopping fetch for ${displayName}.`);
            limitReached = true;
            break;
        }

        const variables = { pageSize: pageSize, lastId: lastId };
        process.stdout.write(`   Fetching page ${page} (Fetched: ${fetchedCount})... `);

        try {
            const data = await request({
                url: endpoint,
                document: gql`${protocolConfig.query}`,
                variables,
                requestHeaders: headers,
                timeout: 30000
            });

            const batch = data[protocolConfig.poolFieldName];

            if (!batch || !Array.isArray(batch)) {
                 process.stdout.write(`❌ Error: Field '${protocolConfig.poolFieldName}' not found or not an array.\n`);
                 console.error("   Response keys received:", Object.keys(data));
                 break;
            }
            if (batch.length === 0) {
                process.stdout.write(`✅ Done (Last page was empty).\n`);
                break;
            }

            const currentBatchSize = batch.length;
            process.stdout.write(`OK (${currentBatchSize} pools).\n`);

            for (const pool of batch) {
                 if (fetchedCount >= fetchLimit) {
                      limitReached = true;
                      break;
                 }

                let token0Addr, token1Addr, symbol0, symbol1;

                // --- Token Address & Symbol Extraction Logic ---
                try {
                    if (pool.token0 && pool.token0.id && pool.token1 && pool.token1.id) {
                        // Standard Uniswap V2/V3, Velodrome (BsBDqD...) style
                        token0Addr = pool.token0.id.toLowerCase();
                        token1Addr = pool.token1.id.toLowerCase();
                        symbol0 = pool.token0.symbol || '???'; // Use '???' if symbol missing
                        symbol1 = pool.token1.symbol || '???';
                    } else if (pool.inputTokens && Array.isArray(pool.inputTokens) && pool.inputTokens.length === 2 && pool.inputTokens[0].id && pool.inputTokens[1].id) {
                        // Older Velodrome V2 style (A4Y1...)
                        token0Addr = pool.inputTokens[0].id.toLowerCase();
                        token1Addr = pool.inputTokens[1].id.toLowerCase();
                        symbol0 = pool.inputTokens[0].symbol || '???';
                        symbol1 = pool.inputTokens[1].symbol || '???';
                    } else if (pool.tokens && Array.isArray(pool.tokens)) {
                        // Balancer V2 style - *FILTER FOR 2-TOKEN POOLS ONLY*
                        if (pool.tokens.length === 2 && pool.tokens[0].address && pool.tokens[1].address) {
                            token0Addr = pool.tokens[0].address.toLowerCase();
                            token1Addr = pool.tokens[1].address.toLowerCase();
                            symbol0 = pool.tokens[0].symbol || '???';
                            symbol1 = pool.tokens[1].symbol || '???';
                        } else {
                            continue; // Skip pools that don't have exactly 2 tokens
                        }
                    } else {
                        // Unknown structure
                        if (pool && pool.id) {
                            console.warn(`   ⚠️ Skipping pool ${pool.id}: Could not extract token pair data structure.`);
                        } else {
                            console.warn(`   ⚠️ Skipping pool with missing ID/structure.`);
                        }
                        continue; // Skip if we can't find the tokens
                    }
                } catch (extractionError) {
                     console.warn(`   ⚠️ Error extracting token data for pool ${pool?.id || 'UNKNOWN'}: ${extractionError.message}. Skipping.`);
                     continue;
                }
                // --- End Token Address & Symbol Extraction ---

                const poolId = pool.id.toLowerCase();

                // Create pair key by sorting addresses alphabetically
                const sortedAddresses = [token0Addr, token1Addr].sort();
                const pairKey = sortedAddresses.join('_');

                // Add pool address to list for this pair
                if (!poolsData[pairKey]) {
                    poolsData[pairKey] = [];
                }
                if (!poolsData[pairKey].includes(poolId)) {
                    poolsData[pairKey].push(poolId);
                }

                // Store symbol mapping if not already present for this pair
                if (!pairSymbolMap[pairKey]) {
                    // Ensure symbol order matches sorted address order
                    const sortedSymbol0 = (token0Addr === sortedAddresses[0]) ? symbol0 : symbol1;
                    const sortedSymbol1 = (token1Addr === sortedAddresses[1]) ? symbol1 : symbol0;
                    pairSymbolMap[pairKey] = `${sortedSymbol0}/${sortedSymbol1}`;
                }

                fetchedCount++; // Increment count *after* successfully processing a pool
            } // End loop through batch

            if (limitReached) {
                console.log(`   ⚠️ Fetch limit (${fetchLimit}) reached during page ${page} processing. Stopping fetch.`);
                break;
            }
            if (currentBatchSize < pageSize) {
                console.log(`   🏁 Fetched last batch (${currentBatchSize} pools).`);
                break;
            }
            lastId = batch[batch.length - 1].id;
            page++;

        } catch (error) {
            process.stdout.write(`❌ Error fetching page ${page}.\n`);
            let errorMessage = error.message;
            // ... (keep existing error handling logic) ...
             if (error.response && error.response.errors) {
                errorMessage = `GraphQL Errors: ${JSON.stringify(error.response.errors)}`;
            } else if (error.response && error.response.status) {
                errorMessage = `HTTP Status ${error.response.status}`;
            }
            console.error(`   Error details: ${errorMessage}`);
            if (errorMessage.includes('401') || errorMessage.includes('Unauthorized')) {
                console.error("   Hint: Check if API Key is valid and has access to this subgraph.");
            } else if (errorMessage.includes('400') || errorMessage.includes('Bad Request')) {
                console.error("   Hint: GraphQL query might be invalid for this subgraph's schema. Verify query in config and on The Graph Explorer.");
            } else if (errorMessage.includes('indexers')) {
                 console.error("   Hint: The subgraph indexers are unhealthy or timing out. Try again later or find an alternative subgraph deployment.");
            } else if (errorMessage.includes('subgraph not found')) {
                 console.error(`   Hint: Subgraph ID '${protocolConfig.subgraphId}' not found on the Gateway. Double-check the ID in the config and on The Graph Explorer.`);
            } else if (errorMessage.includes('Cannot query field')) {
                console.error("   Hint: The fields requested in the query do not exist on the specified entity (e.g., 'pairs', 'pools'). Verify the query and poolFieldName in the config against the subgraph schema.");
            }
            break; // Stop trying on error
        }
    } // End while loop

    console.log(`✅ Finished fetching ${displayName}. Total eligible pools processed: ${fetchedCount}, Unique pairs found: ${Object.keys(poolsData).length}`);
    // Return both maps
    return { poolsData, pairSymbolMap };
}

/**
 * Compares pools from two protocols and saves matches, including symbols.
 * @param {object} poolsA - Pool map from protocol A
 * @param {object} poolsB - Pool map from protocol B
 * @param {object} pairSymbolMapA - Symbol map from protocol A (used as primary source for symbols)
 * @param {string} chainName - Name of the chain
 * @param {string} protocolNameA - Name of protocol A
 * @param {string} protocolNameB - Name of protocol B
 */
 function compareAndSavePools(poolsA, poolsB, pairSymbolMapA, chainName, protocolNameA, protocolNameB) {
    const displayNameA = CONFIG.chains?.[chainName]?.protocols?.[protocolNameA]?.displayName || protocolNameA;
    const displayNameB = CONFIG.chains?.[chainName]?.protocols?.[protocolNameB]?.displayName || protocolNameB;

    console.log(`\n🔄 Comparing ${displayNameA} vs ${displayNameB} on ${chainName}...`);

    const commonPairs = Object.keys(poolsA).filter(pairKey => poolsB[pairKey]);

    console.log(`   Found ${commonPairs.length} common token pairs.`);

    const matchedData = commonPairs.map(pairKey => {
        const [tokenA_addr, tokenB_addr] = pairKey.split('_'); // Addresses sorted alphabetically
        const pairSymbols = pairSymbolMapA[pairKey] || 'UNKNOWN/UNKNOWN'; // Get symbol string

        return {
            tokenA: tokenA_addr,
            tokenB: tokenB_addr,
            pairSymbols: pairSymbols, // Add the human-readable symbols
            [`${protocolNameA}_pools`]: poolsA[pairKey],
            [`${protocolNameB}_pools`]: poolsB[pairKey]
        };
    });

    if (matchedData.length > 0) {
        const outputFileName = `${chainName}_${protocolNameA}_vs_${protocolNameB}_matched.json`;
        const outputPath = path.join(__dirname, outputFileName);
        try {
            fs.writeFileSync(outputPath, JSON.stringify(matchedData, null, 2));
            console.log(`💾 Successfully wrote ${matchedData.length} matched pair details to ${outputPath}`);
        } catch (err) {
            console.error(`❌ Failed to write output file ${outputPath}: ${err.message}`);
        }
    } else {
        console.log(`   ℹ️ No common pairs found. No output file generated for this comparison.`);
    }
}


// --- Main Execution Logic ---
(async () => {
    console.log("--- Starting DEX Pair Scanner ---");

    if (!API_KEY && Object.values(CONFIG.chains).some(chain => Object.values(chain.protocols).some(p => p.useApiKey))) {
        console.warn("\n⚠️ WARNING: Some protocols are configured to use an API key, but GRAPH_API_KEY was not found in the .env file. Access may fail or be limited.\n");
    }

    const comparisonsToRun = [
        { chain: 'optimism', protoA: 'uniswap_v3', protoB: 'velodrome_v2' },
        { chain: 'optimism', protoA: 'uniswap_v3', protoB: 'balancer_v2' }
    ];

    // Cache fetched data to avoid re-fetching (e.g., Uniswap V3)
    const fetchedDataCache = {}; // protocolKey -> { poolsData, pairSymbolMap }

    for (const comparison of comparisonsToRun) {
        const { chain, protoA, protoB } = comparison;
        console.log(`\n===== Running Comparison: ${chain} / ${protoA} vs ${protoB} =====`);

        const chainConfig = CONFIG.chains[chain];
        if (!chainConfig) {
            console.warn(`❓ Chain '${chain}' not found in configuration. Skipping comparison.`);
            continue;
        }
        if (!chainConfig.protocols[protoA] || !chainConfig.protocols[protoB]) {
            console.warn(`❓ One or both protocols (${protoA}, ${protoB}) not defined for chain '${chain}' in config. Skipping.`);
            continue;
        }

        try {
            // Fetch or get from cache
            const keyA = `${chain}-${protoA}`;
            const keyB = `${chain}-${protoB}`;

            if (!fetchedDataCache[keyA]) {
                fetchedDataCache[keyA] = await getAllPools(chain, protoA);
            }
            if (!fetchedDataCache[keyB]) {
                fetchedDataCache[keyB] = await getAllPools(chain, protoB);
            }

            const dataA = fetchedDataCache[keyA];
            const dataB = fetchedDataCache[keyB];

            if (dataA && dataB && dataA.poolsData && dataB.poolsData && Object.keys(dataA.poolsData).length > 0 && Object.keys(dataB.poolsData).length > 0) {
                 // Pass poolsData and the symbol map from the first protocol (A)
                 compareAndSavePools(dataA.poolsData, dataB.poolsData, dataA.pairSymbolMap, chain, protoA, protoB);
            } else {
                 console.error(`   Comparison skipped: One or both protocols failed to return pool data (${protoA}: ${Object.keys(dataA?.poolsData || {}).length}, ${protoB}: ${Object.keys(dataB?.poolsData || {}).length}).`);
            }

        } catch (error) {
             console.error(`❌ Unexpected error during comparison of ${protoA} vs ${protoB} on ${chain}:`, error);
        }
    }

    console.log("\n--- Scanner finished all comparisons. ---");
})();