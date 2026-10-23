import {
  type Sessions,
  order_customer_cancel_order,
  order_customer_list_orders,
  order_customer_place_order,
} from '../../../bench/gen/client.ts';

// A customer places an order, finds it in their list and cancels it.
export default function placeAndCancel(sessions: Sessions): void {
  const placed = order_customer_place_order(sessions.customer, {
    total: { amount: '12.50', currency: 'EUR' },
  });
  order_customer_list_orders(sessions.customer);
  order_customer_cancel_order(sessions.customer, { order: placed.id });
}
