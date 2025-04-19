const { request } = require('graphql-request');
const fs = require('fs');

// Load matched_pools.json
const matchedPoolsFile = 'matched_pools.json';
let matchedPools = [];
try {
  matchedPools = JSON.parse(fs.readFileSync(matchedPoolsFile, 'utf-8'));
} catch (err) {
  console.error(`Failed to load ${matchedPoolsFile}:`, err.message);
  process.exit(1);
}

// Dynamic query based on DEX name
function getQueryForDEX(name) {
  if (name.includes('Uniswap')) {
    return `{
      pools(first: 20, orderBy: volumeUSD, orderDirection: desc) {
        id
        token0 { id symbol }
        token1 { id symbol }
        feeTier
      }
    }`;
  } else if (name.includes('Velodrome')) {
    return `{
      liquidityPools(first: 20, orderBy: totalValueLockedUSD, orderDirection: desc) {
        id
        token0 { id symbol }
        token1 { id symbol }
      }
    }`;
  } else {
    return null;
  }
}

// Fetch function
async function fetchData(endpoint, query) {
  try {
    const data = await request(endpoint, query);
    return data;
  } catch (error) {
    console.error(`Error fetching data from ${endpoint}:`, error.message);
    return null;
  }
}

// Display results side-by-side
function displayFormattedOutput(uniswapData, velodromeData) {
  console.log('\n--- Top 20 Most Queried Pairs ---\n');
  for (let i = 0; i < Math.min(20, uniswapData.length, velodromeData.length); i++) {
    const uniPair = uniswapData[i];
    const veloPair = velodromeData[i];

    const uniToken0 = uniPair?.token0 || {};
    const uniToken1 = uniPair?.token1 || {};
    const veloToken0 = veloPair?.token0 || {};
    const veloToken1 = veloPair?.token1 || {};

    console.log(`Uniswap V3 Op: ${uniToken0.symbol || 'N/A'}/${uniToken1.symbol || 'N/A'}`);
    console.log(`  Token0: ${uniToken0.id || 'N/A'}, Token1: ${uniToken1.id || 'N/A'}, Fee Tier: ${uniPair?.feeTier || 'N/A'}`);
    console.log(`Velodrome V2 Op: ${veloToken0.symbol || 'N/A'}/${veloToken1.symbol || 'N/A'}`);
    console.log(`  Token0: ${veloToken0.id || 'N/A'}, Token1: ${veloToken1.id || 'N/A'}`);
    console.log('-----------------------------------');
  }
}

// Run main script
(async () => {
  const results = [];

  for (let i = 0; i < Math.min(2, matchedPools.length); i++) {
    const dex = matchedPools[i];
    const endpoint = dex.query_url;
    const query = getQueryForDEX(dex.name);

    if (!query) {
      console.error(`No query defined for DEX: ${dex.name}`);
      results.push([]);
      continue;
    }

    console.log(`Fetching data for ${dex.name} (${dex.network})...`);
    const data = await fetchData(endpoint, query);

    let resultSet = [];
    if (data) {
      if (data.pools) resultSet = data.pools;
      else if (data.liquidityPools) resultSet = data.liquidityPools;
    }

    results.push(resultSet);
  }

  if (results.length === 2) {
    console.log('\nFetched Data:', JSON.stringify(results, null, 2));
    displayFormattedOutput(results[0], results[1]);
  } else {
    console.error('Failed to fetch data for one or both DEXs.');
  }
})();
