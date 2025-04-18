const { request, gql } = require('graphql-request');
const fs = require('fs');

// --- Configuration ---
const PAGE_SIZE = 1000;

// ✅ No API key required for Uniswap V3 on Optimism (hosted service)
const UNISWAP_V3_ENDPOINT = 'https://api.thegraph.com/subgraphs/name/ianlapham/uniswap-v3-optimism';

// ✅ Replace YOUR_API_KEY with actual key if using Velodrome V2 via Gateway
const API_KEY = 'd4d5aaa3c6bee7a9dcc827fa462bfb13';
const VELODROME_V2_ENDPOINT = `https://gateway.thegraph.com/api/${API_KEY}/subgraphs/id/A4Y1A82YhSLTn998BVVELC8eWzhi992k4ZitByvssxqA`;

const HEADERS = { Authorization: `Bearer ${API_KEY}` };
const OUTPUT_FILE = 'matched_pools.json';

const UNISWAP_QUERY = gql`
query GetPools($pageSize: Int!, $lastId: ID!) {
  pools(first: $pageSize, where: { id_gt: $lastId }) {
    id
    token0 { id }
    token1 { id }
    feeTier
  }
}
`;

const VELODROME_QUERY = gql`
query GetPools($pageSize: Int!, $lastId: ID!) {
  liquidityPools(first: $pageSize, where: { id_gt: $lastId }) {
    id
    token0 { id }
    token1 { id }
  }
}
`;

async function getAllPools(endpoint, query, protocol, useHeaders = false) {
  console.log(`Fetching pools from ${protocol}...`);
  let lastId = '';
  let poolsData = {};
  let fetchedCount = 0;

  while (true) {
    const variables = { pageSize: PAGE_SIZE, lastId };

    try {
      const data = await request({
        url: endpoint,
        document: query,
        variables,
        requestHeaders: useHeaders ? HEADERS : {}
      });

      const batch = data.pools || data.liquidityPools || [];
      if (!batch.length) break;

      fetchedCount += batch.length;
      console.log(`Fetched ${batch.length} pools from ${protocol}...`);

      for (const pool of batch) {
        const t0 = pool.token0.id.toLowerCase();
        const t1 = pool.token1.id.toLowerCase();
        const poolId = pool.id.toLowerCase();

        const pairKey = [t0, t1].sort().join('_');

        if (!poolsData[pairKey]) poolsData[pairKey] = [];
        poolsData[pairKey].push(poolId);
      }

      if (batch.length < PAGE_SIZE) break;
      lastId = batch[batch.length - 1].id;
    } catch (err) {
      console.error(`Error fetching ${protocol}:`, err.message);
      break;
    }
  }

  console.log(`Finished fetching ${protocol}. Unique pairs: ${Object.keys(poolsData).length}`);
  return poolsData;
}

(async () => {
  const uniswapPools = await getAllPools(UNISWAP_V3_ENDPOINT, UNISWAP_QUERY, 'Uniswap V3');
  const velodromePools = await getAllPools(VELODROME_V2_ENDPOINT, VELODROME_QUERY, 'Velodrome V2', true);

  const commonPairs = Object.keys(uniswapPools).filter(pair => velodromePools[pair]);
  console.log(`\nFound ${commonPairs.length} common token pairs.`);

  const matchedData = commonPairs.map(pair => {
    const [tokenA, tokenB] = pair.split('_');
    return {
      tokenA,
      tokenB,
      uniswapV3_pools: uniswapPools[pair],
      velodrome_pools: velodromePools[pair]
    };
  });

  try {
    fs.writeFileSync(OUTPUT_FILE, JSON.stringify(matchedData, null, 2));
    console.log(`Wrote ${matchedData.length} matched pairs to ${OUTPUT_FILE}`);
  } catch (err) {
    console.error(`Failed to write output: ${err.message}`);
  }
})();
