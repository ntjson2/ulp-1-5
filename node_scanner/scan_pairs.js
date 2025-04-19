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
// Read fetch limit from config, default to Infinity (no limit) if not set
const GLOBAL_FETCH_LIMIT = CONFIG.global?.fetchLimit || Infinity;


// --- Helper Functions ---

/**
 * Fetches pools for a specific protocol, respecting fetch limits.
 */
async function getAllPools(chainName, protocolName) {
    const protocolConfig = CONFIG.chains?.[chainName]?.protocols?.[protocolName];
    const displayName = protocolConfig?.displayName || protocolName;
    const fetchLimit = protocolConfig?.fetchLimit || GLOBAL_FETCH_LIMIT; // Use protocol specific or global limit

    console.log(`\n🌀 Fetching pools for ${displayName} on ${chainName}... (Limit: ${fetchLimit === Infinity ? 'None' : fetchLimit})`);

    if (!protocolConfig || !protocolConfig.subgraphId || !protocolConfig.poolFieldName || !protocolConfig.query) {
        console.error(`❗️ Invalid or incomplete configuration for ${displayName} on ${chainName}. Skipping.`);
        return {}; // Return empty object on bad config
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
    let poolsData = {};
    let fetchedCount = 0;
    let page = 1;
    const pageSize = protocolConfig.pageSize || GLOBAL_PAGE_SIZE;
    let limitReached = false;

    // --- Main Fetch Loop ---
    while (true) {
        // Check if fetch limit has already been reached before making the request
        if (fetchedCount >= fetchLimit) {
            console.log(`   ⚠️ Fetch limit (${fetchLimit}) reached or exceeded. Stopping fetch for ${displayName}.`);
            limitReached = true;
            break; // Exit loop if limit is already met
        }

        const variables = { pageSize: pageSize, lastId: lastId };
        process.stdout.write(`   Fetching page ${page} (Fetched: ${fetchedCount})... `);

        try {
            const data = await request({
                url: endpoint,
                document: gql`${protocolConfig.query}`,
                variables,
                requestHeaders: headers,
                timeout: 30000 // Increased timeout slightly
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

            // Process the batch
            for (const pool of batch) {
                 // --- Early exit from loop if limit reached during processing ---
                 if (fetchedCount >= fetchLimit) {
                      limitReached = true;
                      break; // Stop processing this batch
                 }
                // --- End early exit check ---


                let token0Addr, token1Addr;
                if (pool.token0 && pool.token0.id && pool.token1 && pool.token1.id) {
                    token0Addr = pool.token0.id.toLowerCase();
                    token1Addr = pool.token1.id.toLowerCase();
                } else if (pool.inputTokens && Array.isArray(pool.inputTokens) && pool.inputTokens.length === 2 && pool.inputTokens[0].id && pool.inputTokens[1].id) {
                    token0Addr = pool.inputTokens[0].id.toLowerCase();
                    token1Addr = pool.inputTokens[1].id.toLowerCase();
                } else {
                    // Adding a check for pool existence before warning
                    if (pool && pool.id) {
                        console.warn(`   ⚠️ Skipping pool ${pool.id}: Could not extract token pair addresses.`);
                    } else {
                        console.warn(`   ⚠️ Skipping pool with missing ID: Could not extract token pair addresses.`);
                    }
                    continue;
                }

                const poolId = pool.id.toLowerCase();
                const pairKey = [token0Addr, token1Addr].sort().join('_');

                if (!poolsData[pairKey]) {
                    poolsData[pairKey] = [];
                }
                if (!poolsData[pairKey].includes(poolId)) {
                    poolsData[pairKey].push(poolId);
                }
                 // Increment count *after* successfully processing a pool
                 fetchedCount++;
            } // End loop through batch

            // Exit loop if limit was reached during batch processing
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
            if (error.response && error.response.errors) {
                errorMessage = `GraphQL Errors: ${JSON.stringify(error.response.errors)}`;
            } else if (error.response && error.response.status) {
                errorMessage = `HTTP Status ${error.response.status}`;
            }
            console.error(`   Error details: ${errorMessage}`);
            // Add specific hints based on errors
            if (errorMessage.includes('401') || errorMessage.includes('Unauthorized')) {
                console.error("   Hint: Check if API Key is valid and has access to this subgraph.");
            } else if (errorMessage.includes('400') || errorMessage.includes('Bad Request')) {
                console.error("   Hint: GraphQL query might be invalid for this subgraph's schema. Verify query in config and on The Graph Explorer.");
            } else if (errorMessage.includes('indexers')) { // Handle indexer errors explicitly
                 console.error("   Hint: The subgraph indexers are unhealthy or timing out. Try again later or find an alternative subgraph deployment.");
            } else if (errorMessage.includes('subgraph not found')) { // Handle not found errors
                 console.error(`   Hint: Subgraph ID '${protocolConfig.subgraphId}' not found on the Gateway. Double-check the ID in the config and on The Graph Explorer.`);
            }
            break; // Stop trying on error
        }
    } // End while loop

    console.log(`✅ Finished fetching ${displayName}. Total processed: ${fetchedCount}, Unique pairs found: ${Object.keys(poolsData).length}`);
    return poolsData;
}

/**
 * Compares pools from two protocols and saves matches.
 */
 function compareAndSavePools(poolsA, poolsB, chainName, protocolNameA, protocolNameB) {
    const displayNameA = CONFIG.chains?.[chainName]?.protocols?.[protocolNameA]?.displayName || protocolNameA;
    const displayNameB = CONFIG.chains?.[chainName]?.protocols?.[protocolNameB]?.displayName || protocolNameB;

    console.log(`\n🔄 Comparing ${displayNameA} vs ${displayNameB} on ${chainName}...`);

    const commonPairs = Object.keys(poolsA).filter(pairKey => poolsB[pairKey]); // Find pairKeys present in both

    console.log(`   Found ${commonPairs.length} common token pairs.`);

    const matchedData = commonPairs.map(pairKey => {
        const [tokenA, tokenB] = pairKey.split('_');
        return {
            tokenA,
            tokenB,
            [`${protocolNameA}_pools`]: poolsA[pairKey], // Use the actual protocol key name
            [`${protocolNameB}_pools`]: poolsB[pairKey]
        };
    });

    if (matchedData.length > 0) {
        // Use protocol names in filename for clarity
        const outputFileName = `${chainName}_${protocolNameA}_vs_${protocolNameB}_matched.json`;
        const outputPath = path.join(__dirname, outputFileName);
        try {
            fs.writeFileSync(outputPath, JSON.stringify(matchedData, null, 2)); // Pretty print
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

    // Define which cross-protocol comparisons to run on which chains
    // Updated comparisons list
    const comparisonsToRun = [
        { chain: 'optimism', protoA: 'uniswap_v3', protoB: 'velodrome_v2' }, // Using new Velo V2 subgraph
        { chain: 'optimism', protoA: 'uniswap_v3', protoB: 'aerodrome' }    // Added Uni V3 vs Aerodrome
        // You could also add:
        // { chain: 'optimism', protoA: 'velodrome_v2', protoB: 'aerodrome' }
    ];

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

        // Fetch pools for both protocols sequentially
        try {
            // Fetching sequentially is safer for API limits and resource usage
            const poolsA = await getAllPools(chain, protoA);
            const poolsB = await getAllPools(chain, protoB);

             // Only compare if both fetches were successful (returned objects) and returned some pools
            if (poolsA && poolsB && Object.keys(poolsA).length > 0 && Object.keys(poolsB).length > 0) {
                 compareAndSavePools(poolsA, poolsB, chain, protoA, protoB);
            } else {
                 console.error(`   Comparison skipped: One or both protocols failed to return pool data (${protoA}: ${Object.keys(poolsA || {}).length}, ${protoB}: ${Object.keys(poolsB || {}).length}).`);
            }

        } catch (error) {
             console.error(`❌ Unexpected error during comparison of ${protoA} vs ${protoB} on ${chain}:`, error);
        }
    }

    console.log("\n--- Scanner finished all comparisons. ---");
})();