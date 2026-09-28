// Based on ranxi2001/sub2api bf782999, with the user's no-search/tool instruction.
export const CANDY_PROMPT = `禁止联网搜索或调用工具，回答我：在一个黑色的袋子里放有三种口味的糖果，每种糖果有两种不同的形状（圆形和五角星形，不同的形状靠手感可以分辨）。现已知不同口味的糖和不同形状的数量统计如下表。参赛者需要在活动前决定摸出的糖果数目，那么，最少取出多少个糖果才能保证手中同时拥有不同形状的苹果味和桃子味的糖？（同时手中有圆形苹果味匹配五角星桃子味糖果，或者有圆形桃子味匹配五角星苹果味糖果都满足要求）

苹果味 桃子味 西瓜味
圆形 7 9 8
五角星形 7 6 4`

export const CANDY_REFERENCE_ANSWER = '21'

export const DEFAULT_JUDGE_PROMPT = '只比较两个值：reference_answer（参考值）与 candidate_answer（候选值）。数值相同或语义等价返回 correct，明确不同返回 incorrect，无法确定返回 unknown。忽略单位、标点和措辞差异，不要引用或推测题目内容。'
