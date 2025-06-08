from neutron_lib import policy as base
from oslo_policy import policy


rules = [
    policy.DocumentedRuleDefault(
        'create_port_steering',
        base.RULE_ANY,
        'Create a port steering',
        [
            {
                'method': 'POST',
                'path': '/port_steerings',
            },
        ]
    ),
    policy.DocumentedRuleDefault(
        'update_port_steering',
        base.RULE_ADMIN_OR_OWNER,
        'Update a port steering',
        [
            {
                'method': 'PUT',
                'path': '/port_steerings/{id}',
            },
        ]
    ),
    policy.DocumentedRuleDefault(
        'delete_port_steering',
        base.RULE_ADMIN_OR_OWNER,
        'Delete a port steering',
        [
            {
                'method': 'DELETE',
                'path': '/port_steerings/{id}',
            },
        ]
    ),
    policy.DocumentedRuleDefault(
        'get_port_steering',
        base.RULE_ADMIN_OR_OWNER,
        'Get port steerings',
        [
            {
                'method': 'GET',
                'path': '/port_steerings',
            },
            {
                'method': 'GET',
                'path': '/port_steerings/{id}',
            },
        ]
    ),
]


def list_rules():
    return rules
